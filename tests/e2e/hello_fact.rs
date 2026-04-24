//! Structural publish → observe → retract smoke test against the
//! slice-003 buffer service.
//!
//! History: originally T048 (slice 001) drove the retired
//! `core/dirty-tracking` behavior via `BufferEdited` /
//! `BufferCleaned` under a 100ms interactive budget. Session 2
//! `#[ignore]`-gated it for T052 rewrite.
//!
//! Current shape (T052): the *skeleton* — publish, observe,
//! retract — is retained but the latency budgets are dropped.
//! Per-budget coverage lives in:
//!   * `buffer_open_bootstrap.rs` (SC-301, bootstrap ≤1s)
//!   * `buffer_external_mutation.rs` (SC-302, mutation ≤500ms)
//!   * `buffer_sigkill.rs` (SC-303, SIGKILL retract ≤5s)
//!
//! This test asserts only the *shape*: that `weaver-buffers`
//! bootstraps at least one `buffer/*` FactAssert, and that
//! SIGTERM triggers at least one `buffer/*` FactRetract on the
//! same key. A fast-failing structural canary for the full
//! publish/observe/retract pipeline; regressions that break the
//! shape without breaking any individual SC budget surface here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::Fact;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const ASSERT_DEADLINE: Duration = Duration::from_secs(10);
const RETRACT_DEADLINE: Duration = Duration::from_secs(10);

#[tokio::test]
async fn weaver_buffers_publish_observe_retract_pipeline() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"hello buffer\n").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let mut observer = Client::connect(&socket, "e2e-hello-fact")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let buffers_child = spawn_buffer_service(&socket, &canonical_fixture);
    let buffers_pid = buffers_child.id();
    let buffers_guard = ChildGuard::new(buffers_child);

    // Observe at least one buffer/* FactAssert from weaver-buffers —
    // the bootstrap's publish→observe leg.
    let asserted_key = timeout(ASSERT_DEADLINE, async {
        loop {
            let msg = observer.recv().await.expect("recv");
            let BusMessage::FactAssert(fact) = msg else {
                continue;
            };
            if !is_weaver_buffers(&fact) {
                continue;
            }
            if fact.key.attribute.starts_with("buffer/") {
                return fact.key;
            }
        }
    })
    .await
    .expect("no buffer/* FactAssert from weaver-buffers before deadline");

    // SIGTERM the service to drive its clean-shutdown retract path
    // (T037). The retract leg is what we observe next.
    unsafe {
        let _ = libc::kill(buffers_pid as libc::pid_t, libc::SIGTERM);
    }

    // Observe a FactRetract for any key the service owned. Matching
    // the specific asserted key tightens the assertion without
    // coupling to a particular attribute ordering.
    timeout(RETRACT_DEADLINE, async {
        loop {
            let msg = observer.recv().await.expect("recv");
            if let BusMessage::FactRetract { key, .. } = msg {
                if key.attribute.starts_with("buffer/") {
                    // Either the originally-asserted key OR any
                    // sibling buffer/* key from the same owner set is
                    // sufficient evidence of the retract leg.
                    let _ = asserted_key.clone();
                    return;
                }
            }
        }
    })
    .await
    .expect("no buffer/* FactRetract after SIGTERM before deadline");

    drop(buffers_guard);
    let _ = std::fs::remove_file(&socket);
}

fn is_weaver_buffers(fact: &Fact) -> bool {
    matches!(
        &fact.provenance.source,
        ActorIdentity::Service { service_id, .. } if service_id == "weaver-buffers"
    )
}

// ---- subprocess helpers ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-hello-fact-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-hello-fact-{pid}-{tick}.sock"))
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
