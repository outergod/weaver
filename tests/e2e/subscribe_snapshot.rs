//! Snapshot-on-subscribe: a client that subscribes *after*
//! `weaver-buffers` has already bootstrapped still receives the
//! current state as FactAssert message(s). Regresses the review
//! finding that noted subscribers missing already-asserted facts.
//!
//! History: originally part of slice 001's snapshot contract,
//! driven by the retired `core/dirty-tracking` behavior. Session
//! 2 `#[ignore]`-gated it pending T054 rewrite.
//!
//! Slice-003 shape: wait for `weaver-buffers` to publish its
//! bootstrap (observed out-of-band via `weaver status`), then
//! connect a fresh client and confirm the snapshot-replay on
//! subscribe delivers the already-asserted buffer/* facts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_buffers::model::buffer_entity_ref;
use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::Fact;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const BOOTSTRAP_WAIT: Duration = Duration::from_secs(10);
const SNAPSHOT_WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn late_subscriber_receives_weaver_buffers_snapshot() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"hello buffer\n").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");
    let expected_entity = buffer_entity_ref(&canonical_fixture).as_u64();

    let _buffers = ChildGuard::new(spawn_buffer_service(&socket, &canonical_fixture));

    // Wait for bootstrap to complete via the out-of-band
    // `weaver status` probe. No subscriber is connected during this
    // window — that is the whole point: the late subscriber below
    // must still see the fact via the snapshot-on-subscribe path.
    let core_bin = bin_path("weaver");
    wait_for_buffer_dirty(&core_bin, &socket, expected_entity).await;

    // Now connect the late subscriber — deliberately AFTER the
    // bootstrap has landed.
    let mut subscriber = Client::connect(&socket, "e2e-late-subscriber")
        .await
        .expect("late subscriber connect");
    subscriber
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // The snapshot replay must deliver a buffer/* FactAssert for the
    // expected entity, attributed to weaver-buffers.
    let fact = timeout(SNAPSHOT_WAIT, async {
        loop {
            let msg = subscriber.recv().await.expect("recv");
            let BusMessage::FactAssert(fact) = msg else {
                continue;
            };
            if fact.key.entity.as_u64() != expected_entity {
                continue;
            }
            if !fact.key.attribute.starts_with("buffer/") {
                continue;
            }
            if !is_weaver_buffers(&fact) {
                continue;
            }
            return fact;
        }
    })
    .await
    .expect("late subscriber did not receive a weaver-buffers snapshot FactAssert");

    match fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => {
            assert_eq!(
                service_id, "weaver-buffers",
                "snapshot fact must be attributed to weaver-buffers; got {service_id:?}"
            );
        }
        other => panic!("snapshot fact must be service-authored; got {other:?}"),
    }

    let _ = std::fs::remove_file(&socket);
}

fn is_weaver_buffers(fact: &Fact) -> bool {
    matches!(
        &fact.provenance.source,
        ActorIdentity::Service { service_id, .. } if service_id == "weaver-buffers"
    )
}

/// Poll `weaver status --output=json` until `buffer/dirty` appears
/// for the expected entity. Mirrors the probe used in
/// `buffer_inspect_attribution.rs`.
async fn wait_for_buffer_dirty(core_bin: &Path, socket: &Path, expected_entity: u64) {
    let deadline = Instant::now() + BOOTSTRAP_WAIT;
    loop {
        if Instant::now() >= deadline {
            panic!("buffer/dirty fact did not appear within {BOOTSTRAP_WAIT:?}");
        }
        let out = Command::new(core_bin)
            .args(["--socket", socket.to_str().unwrap(), "status", "-o", "json"])
            .output()
            .expect("status runs");
        if out.status.success()
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            && let Some(facts) = v.get("facts").and_then(|f| f.as_array())
        {
            for f in facts {
                let attribute = f
                    .get("key")
                    .and_then(|k| k.get("attribute"))
                    .and_then(|a| a.as_str());
                let entity = f
                    .get("key")
                    .and_then(|k| k.get("entity"))
                    .and_then(|e| e.as_u64());
                if attribute == Some("buffer/dirty") && entity == Some(expected_entity) {
                    return;
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

// ---- subprocess helpers ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-snapshot-{pid}-{tick}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success());
    bin_path("weaver")
}

fn build_buffer_service_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args([
            "build",
            "--quiet",
            "-p",
            "weaver-buffers",
            "--bin",
            "weaver-buffers",
        ])
        .status()
        .expect("cargo build weaver-buffers");
    assert!(status.success());
    bin_path("weaver-buffers")
}

fn bin_path(name: &str) -> PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
        });
    target.join("debug").join(name)
}

fn spawn_core(socket: &Path) -> std::process::Child {
    let bin = build_weaver_binary();
    Command::new(&bin)
        .arg("run")
        .arg("--socket")
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver")
}

fn spawn_buffer_service(socket: &Path, path: &Path) -> std::process::Child {
    let bin = build_buffer_service_binary();
    Command::new(&bin)
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver-buffers")
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
    std::env::temp_dir().join(format!("weaver-snapshot-{pid}-{tick}.sock"))
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
            unsafe {
                let _ = libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(100));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
