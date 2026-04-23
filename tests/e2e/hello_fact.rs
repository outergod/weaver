//! T048 (slice 001) — end-to-end scenario that drove the
//! `core/dirty-tracking` behavior via `BufferEdited` / `BufferCleaned`.
//!
//! Slice 003 retired both the behavior and the event variants; this
//! test is `#[ignore]`-gated pending T052 rewrite to drive
//! `weaver-buffers <fixture>` + external-mutation instead.
//!
//! Reference: `specs/003-buffer-service/tasks.md` T052.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use uuid::Uuid;
use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const INTERACTIVE_BUDGET: Duration = Duration::from_millis(100);
const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
#[ignore = "rewrite under T052 in Phase 4 to drive weaver-buffers + external mutation"]
async fn buffer_edited_then_cleaned_round_trips_via_bus() {
    let socket = unique_socket_path();
    let _guard = ChildGuard::new(spawn_weaver(&socket));

    wait_for_socket(&socket).await;

    let mut client = Client::connect(&socket, "e2e-test")
        .await
        .expect("connect to bus");
    client
        .subscribe(SubscribePattern::FamilyPrefix("buffer/".into()))
        .await
        .expect("subscribe to buffer/*");

    // Publish BufferOpen and verify FactAssert within 100 ms.
    let edit_event_id = EventId::new(now_ns());
    client
        .send(&BusMessage::Event(build_event(
            edit_event_id,
            EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            "buffer/open",
        )))
        .await
        .expect("send BufferOpen");

    let assert_start = Instant::now();
    let assert_msg = wait_for_fact_assert(&mut client).await;
    let assert_elapsed = assert_start.elapsed();

    match assert_msg {
        BusMessage::FactAssert(fact) => {
            assert_eq!(
                fact.key,
                FactKey::new(EntityRef::new(1), "buffer/dirty"),
                "wrong fact key asserted",
            );
            assert_eq!(fact.value, FactValue::Bool(true));
            assert_eq!(
                fact.provenance.source,
                ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
            );
            assert_eq!(fact.provenance.causal_parent, Some(edit_event_id));
        }
        other => panic!("expected FactAssert, got {other:?}"),
    }
    assert!(
        assert_elapsed <= INTERACTIVE_BUDGET,
        "FactAssert latency exceeded budget: {:?} > {:?}",
        assert_elapsed,
        INTERACTIVE_BUDGET,
    );

    // Publish a second BufferOpen and verify FactRetract within 100 ms.
    let clean_event_id = EventId::new(now_ns());
    client
        .send(&BusMessage::Event(build_event(
            clean_event_id,
            EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            "buffer/open",
        )))
        .await
        .expect("send BufferOpen");

    let retract_start = Instant::now();
    let retract_msg = wait_for_fact_retract(&mut client).await;
    let retract_elapsed = retract_start.elapsed();

    match retract_msg {
        BusMessage::FactRetract { key, .. } => {
            assert_eq!(key, FactKey::new(EntityRef::new(1), "buffer/dirty"));
        }
        other => panic!("expected FactRetract, got {other:?}"),
    }
    assert!(
        retract_elapsed <= INTERACTIVE_BUDGET,
        "FactRetract latency exceeded budget: {:?} > {:?}",
        retract_elapsed,
        INTERACTIVE_BUDGET,
    );
}

async fn wait_for_fact_assert(client: &mut Client) -> BusMessage {
    wait_for(client, |m| matches!(m, BusMessage::FactAssert(_))).await
}

async fn wait_for_fact_retract(client: &mut Client) -> BusMessage {
    wait_for(client, |m| matches!(m, BusMessage::FactRetract { .. })).await
}

async fn wait_for<F>(client: &mut Client, pred: F) -> BusMessage
where
    F: Fn(&BusMessage) -> bool,
{
    let deadline = Duration::from_secs(5);
    let t = timeout(deadline, async {
        loop {
            let msg = client.recv().await.expect("bus recv");
            if pred(&msg) {
                return msg;
            }
        }
    })
    .await;
    t.expect("deadline elapsed waiting for expected bus message")
}

fn build_event(id: EventId, payload: EventPayload, name: &str) -> Event {
    Event {
        id,
        name: name.into(),
        target: Some(EntityRef::new(1)),
        payload,
        provenance: Provenance::new(
            ActorIdentity::service("e2e-publisher", Uuid::new_v4()).unwrap(),
            id.as_u64(),
            None,
        )
        .unwrap(),
    }
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver binary");
    assert!(status.success(), "cargo build failed");
    weaver_bin_path()
}

fn weaver_bin_path() -> PathBuf {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("workspace root")
                .join("target")
        });
    target_dir.join("debug").join("weaver")
}

fn spawn_weaver(socket: &Path) -> std::process::Child {
    let bin = build_weaver_binary();
    Command::new(&bin)
        .arg("run")
        .arg("--socket")
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver subprocess")
}

async fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while !socket.exists() {
        if start.elapsed() > SOCKET_WAIT_TIMEOUT {
            panic!("weaver socket did not appear within {SOCKET_WAIT_TIMEOUT:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
    // Additional short settle so the accept loop is actually accepting.
    sleep(Duration::from_millis(50)).await;
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    std::env::temp_dir().join(format!("weaver-e2e-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// RAII guard that owns the spawned `weaver` subprocess.
///
/// On `Drop` (including panic unwind), it sends SIGTERM, briefly
/// waits for the `cli::run_core` signal handler to unlink the
/// socket, then falls back to SIGKILL (`Child::kill` is a no-op if
/// the process already exited) and always `wait()`s to reap the
/// zombie. Owning the `Child` directly satisfies clippy's
/// `zombie_processes` lint: every code path, including panics,
/// flows through this destructor.
struct ChildGuard {
    child: Option<std::process::Child>,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = nix_signal(child.id(), libc::SIGTERM);
            std::thread::sleep(Duration::from_millis(100));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn nix_signal(pid: u32, sig: libc::c_int) -> std::io::Result<()> {
    // Safety: `libc::kill` is a syscall; arguments are scalars.
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
