//! T073 (slice 001) — end-to-end disconnect scenario that drove the
//! `core/dirty-tracking` behavior via `BufferEdited`.
//!
//! Slice 003 retired the behavior and the event variant; this test is
//! `#[ignore]`-gated pending T053 rewrite to drive `weaver-buffers` as
//! the service whose disconnect is being observed.
//!
//! Reference: `specs/003-buffer-service/tasks.md` T053.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use uuid::Uuid;
use weaver_core::bus::client::{Client, ClientError};
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const DISCONNECT_BUDGET: Duration = Duration::from_secs(5);
const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
#[ignore = "rewrite under T053 in Phase 4 to drive weaver-buffers service disconnect"]
async fn sigkill_surfaces_disconnect_within_budget_without_panic() {
    let socket = unique_socket_path();
    let guard = ChildGuard::new(spawn_weaver(&socket));
    let child_pid = guard.pid();

    wait_for_socket(&socket).await;

    let mut client = Client::connect(&socket, "e2e-disconnect")
        .await
        .expect("connect to bus");
    client
        .subscribe(SubscribePattern::FamilyPrefix("buffer/".into()))
        .await
        .expect("subscribe");

    // Prime the connection with a FactAssert so the stale-view path is
    // non-trivial.
    let edit_id = EventId::new(now_ns());
    client
        .send(&BusMessage::Event(Event {
            id: edit_id,
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(
                ActorIdentity::service("e2e-publisher", Uuid::new_v4()).unwrap(),
                edit_id.as_u64(),
                None,
            )
            .unwrap(),
        }))
        .await
        .expect("send BufferOpen");

    let _fact = timeout(Duration::from_secs(2), async {
        loop {
            let msg = client.recv().await.expect("recv");
            if let BusMessage::FactAssert(f) = msg {
                return f;
            }
        }
    })
    .await
    .expect("FactAssert did not arrive before SIGKILL");

    // Hard-kill the core.
    let _ = nix_signal(child_pid, libc::SIGKILL);

    // Observe disconnect — either stream EOF (Ok(...) loop breaks to Err)
    // or immediate Err — within the 5 s budget. Must not panic.
    let disconnect_start = Instant::now();
    let outcome: Result<(), ClientError> = timeout(DISCONNECT_BUDGET, async {
        loop {
            match client.recv().await {
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .expect("disconnect did not surface within 5 seconds");

    let disconnect_elapsed = disconnect_start.elapsed();
    assert!(
        outcome.is_err(),
        "recv loop must terminate with an error on disconnect"
    );
    assert!(
        disconnect_elapsed <= DISCONNECT_BUDGET,
        "disconnect surfaced too slowly: {disconnect_elapsed:?}",
    );

    // `ChildGuard::drop` will reap the already-SIGKILL'd child.
    drop(guard);
    let _ = std::fs::remove_file(&socket);
}

/// RAII guard that owns the spawned `weaver` subprocess.
///
/// On `Drop` (including panic unwind), it sends SIGTERM, briefly
/// waits, then falls back to SIGKILL (`Child::kill`) and always
/// `wait()`s to reap the zombie. Owning the `Child` directly satisfies
/// clippy's `zombie_processes` lint: every code path, including
/// panics, flows through this destructor.
struct ChildGuard {
    child: Option<std::process::Child>,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().map(|c| c.id()).unwrap_or(0)
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
    sleep(Duration::from_millis(50)).await;
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    std::env::temp_dir().join(format!("weaver-e2e-disconnect-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn nix_signal(pid: u32, sig: libc::c_int) -> std::io::Result<()> {
    // Safety: `libc::kill` is a syscall with scalar args.
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
