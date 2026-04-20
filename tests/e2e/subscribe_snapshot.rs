//! Snapshot-on-subscribe: a client that subscribes *after* a fact has
//! been asserted still receives the current state as FactAssert
//! message(s). Regresses the review finding that noted subscribers
//! missing already-asserted facts.
//!
//! Contract reference: `specs/001-hello-fact/contracts/bus-messages.md`
//! §`FactAssert` — "On reconnect, subscribers receive the current
//! snapshot of subscribed fact families followed by missed deltas."

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn subscribe_replays_current_facts_before_live_updates() {
    let socket = unique_socket_path();
    let mut core = spawn_weaver(&socket);
    let core_pid = core.id();
    wait_for_socket(&socket).await;

    // Step 1: an earlier client publishes `buffer/edited` and disconnects.
    // The fact `buffer/dirty(1) = true` is now in the fact store, but
    // no subscriber was connected when it landed.
    {
        let mut publisher = Client::connect(&socket, "e2e-publisher")
            .await
            .expect("publisher connect");
        let edit_id = EventId::new(now_ns());
        publisher
            .send(&BusMessage::Event(Event {
                id: edit_id,
                name: "buffer/edited".into(),
                target: Some(EntityRef::new(1)),
                payload: EventPayload::BufferEdited,
                provenance: Provenance::new(
                    SourceId::External("e2e-publisher".into()),
                    edit_id.as_u64(),
                    None,
                )
                .unwrap(),
            }))
            .await
            .expect("send BufferEdited");
        // Give the dispatcher a beat to process the event before we
        // disconnect. Without this the TCP close could race the
        // dispatcher's fact-store mutation.
        sleep(Duration::from_millis(50)).await;
    }

    // Step 2: a fresh client connects and subscribes. It should
    // receive `FactAssert(buffer/dirty=true)` from the snapshot even
    // though no new event is published on this connection.
    let mut subscriber = Client::connect(&socket, "e2e-subscriber")
        .await
        .expect("subscriber connect");
    subscriber
        .subscribe(SubscribePattern::FamilyPrefix("buffer/".into()))
        .await
        .expect("subscribe");

    let msg = timeout(Duration::from_secs(2), async {
        loop {
            let m = subscriber.recv().await.expect("recv");
            if let BusMessage::FactAssert(f) = m {
                return f;
            }
            // Ignore Lifecycle/other messages.
        }
    })
    .await
    .expect("snapshot FactAssert did not arrive on subscribe");

    assert_eq!(
        msg.key,
        FactKey::new(EntityRef::new(1), "buffer/dirty"),
        "snapshot replay should deliver the already-asserted fact",
    );
    assert_eq!(msg.value, FactValue::Bool(true));
    assert_eq!(
        msg.provenance.source,
        SourceId::Behavior(BehaviorId::new("core/dirty-tracking")),
    );

    // Clean up.
    let _ = nix_signal(core_pid, libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(100));
    let _ = core.kill();
    let _ = core.wait();
    let _ = std::fs::remove_file(&socket);
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
            panic!("socket did not appear within {SOCKET_WAIT_TIMEOUT:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
    sleep(Duration::from_millis(50)).await;
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    std::env::temp_dir().join(format!("weaver-subscribe-snapshot-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn nix_signal(pid: u32, sig: libc::c_int) -> std::io::Result<()> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
