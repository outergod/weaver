//! F8 regression: the bus listener must reject client-originated
//! `FactAssert` whose provenance is not `ActorIdentity::Service`.
//!
//! Pre-slice-002 the listener rejected all client `FactAssert`
//! frames as a protocol error. Slice 002 opened that path for
//! service publishers (git-watcher et al.), but initially accepted
//! any provenance — letting a client impersonate `Core` or
//! `Behavior` and overwrite in-core `buffer/*` / lifecycle facts.
//! This test proves the listener now refuses non-`Service`
//! provenance with a structured `Error { category: "unauthorized" }`
//! and leaves the fact store untouched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};
use uuid::Uuid;

use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::BehaviorId;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn rejects_core_provenance_fact_assert() {
    let socket = unique_socket_path();
    let _guard = ChildGuard::new(spawn_weaver(&socket));
    wait_for_socket(&socket).await;

    let mut client = Client::connect(&socket, "e2e-impersonator")
        .await
        .expect("connect to bus");

    let forged = Fact {
        key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
        value: FactValue::Bool(true),
        provenance: Provenance::new(ActorIdentity::Core, now_ns(), None)
            .expect("well-formed provenance"),
    };
    client
        .send(&BusMessage::FactAssert(forged))
        .await
        .expect("send forged FactAssert");

    let err = wait_for_error(&mut client).await;
    match err {
        BusMessage::Error(e) => {
            assert_eq!(e.category, "unauthorized", "wrong error category: {e:?}");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // And: the fact store must not contain buffer/dirty — confirm by
    // subscribing to buffer/* and observing zero snapshot entries.
    client
        .subscribe(SubscribePattern::FamilyPrefix("buffer/".into()))
        .await
        .expect("subscribe");
    // Short grace for snapshot replay (empty here). Any FactAssert
    // arriving in this window means the forged write slipped through.
    let window = sleep(Duration::from_millis(100));
    tokio::pin!(window);
    loop {
        tokio::select! {
            () = &mut window => break,
            msg = client.recv() => {
                let msg = msg.expect("recv");
                if let BusMessage::FactAssert(f) = msg {
                    panic!("fact store leaked a FactAssert after forged write: {f:?}");
                }
            }
        }
    }
}

#[tokio::test]
async fn retract_provenance_is_synthesized_server_side() {
    // F11 regression: a legitimate owner retracting their own fact
    // must not be able to forge the retraction's attribution. The
    // dispatcher synthesizes `source` and `timestamp_ns` from the
    // fact's stored provenance; only `causal_parent` is retained
    // from the client frame (as a correlation hint).
    let socket = unique_socket_path();
    let _guard = ChildGuard::new(spawn_weaver(&socket));
    wait_for_socket(&socket).await;

    // Client A: publish as a service.
    let mut publisher = Client::connect(&socket, "e2e-publisher")
        .await
        .expect("connect publisher");
    let instance = Uuid::new_v4();
    let svc_identity =
        ActorIdentity::service("test-publisher", instance).expect("valid service identity");
    let key = FactKey::new(EntityRef::new(42), "test/marker");
    let fact = Fact {
        key: key.clone(),
        value: FactValue::Bool(true),
        provenance: Provenance::new(svc_identity.clone(), now_ns(), None).unwrap(),
    };
    publisher
        .send(&BusMessage::FactAssert(fact))
        .await
        .expect("send FactAssert");

    // Client B: subscribe so we can observe the retract provenance.
    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("connect observer");
    observer
        .subscribe(SubscribePattern::FamilyPrefix("test/".into()))
        .await
        .expect("subscribe");
    // Drain the snapshot.
    loop {
        let msg = timeout(Duration::from_millis(500), observer.recv())
            .await
            .expect("snapshot timeout")
            .expect("snapshot recv");
        if let BusMessage::FactAssert(f) = msg {
            if f.key == key {
                break;
            }
        }
    }

    // Publisher retracts with forged Core provenance.
    let forged_parent = weaver_core::types::ids::EventId::new(0xDEADBEEF);
    let forged_retract_prov =
        Provenance::new(ActorIdentity::Core, now_ns(), Some(forged_parent)).unwrap();
    publisher
        .send(&BusMessage::FactRetract {
            key: key.clone(),
            provenance: forged_retract_prov,
        })
        .await
        .expect("send FactRetract");

    // Observe the broadcast retract: its source must be the original
    // asserter's Service identity, NOT the forged Core. The
    // causal_parent hint is allowed to survive.
    let retract = timeout(Duration::from_secs(5), async {
        loop {
            let msg = observer.recv().await.expect("recv");
            if let BusMessage::FactRetract { key: k, provenance } = msg {
                if k == key {
                    return provenance;
                }
            }
        }
    })
    .await
    .expect("deadline waiting for retract");

    assert_eq!(
        retract.source, svc_identity,
        "retract provenance.source must be server-synthesized from the original asserter"
    );
    assert_eq!(
        retract.causal_parent,
        Some(forged_parent),
        "causal_parent is a correlation hint and survives",
    );
}

#[tokio::test]
async fn rejects_behavior_provenance_fact_assert() {
    let socket = unique_socket_path();
    let _guard = ChildGuard::new(spawn_weaver(&socket));
    wait_for_socket(&socket).await;

    let mut client = Client::connect(&socket, "e2e-impersonator")
        .await
        .expect("connect to bus");

    let forged = Fact {
        key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
        value: FactValue::Bool(true),
        provenance: Provenance::new(
            ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
            now_ns(),
            None,
        )
        .expect("well-formed provenance"),
    };
    client
        .send(&BusMessage::FactAssert(forged))
        .await
        .expect("send forged FactAssert");

    let err = wait_for_error(&mut client).await;
    match err {
        BusMessage::Error(e) => assert_eq!(e.category, "unauthorized"),
        other => panic!("expected Error, got {other:?}"),
    }
}

async fn wait_for_error(client: &mut Client) -> BusMessage {
    let deadline = Duration::from_secs(5);
    timeout(deadline, async {
        loop {
            let msg = client.recv().await.expect("bus recv");
            if matches!(msg, BusMessage::Error(_)) {
                return msg;
            }
        }
    })
    .await
    .expect("deadline elapsed waiting for Error")
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
    std::env::temp_dir().join(format!("weaver-e2e-identity-guard-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

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
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
