//! T047 — SC-302 e2e: external-mutation latency.
//!
//! Three-process scenario (core + `weaver-buffers` + test-client):
//!
//! 1. Spawn core; wait for bus socket.
//! 2. Write a fixture file, subscribe the observer, spawn
//!    `weaver-buffers` against the fixture, wait for the bootstrap
//!    `buffer/dirty=false` to arrive.
//! 3. Mutate the fixture on disk; start stopwatch; wait for
//!    `buffer/dirty=true`; record elapsed.
//! 4. Revert the fixture; start stopwatch; wait for
//!    `buffer/dirty=false`; record elapsed.
//!
//! Surfaces both observed timings via stderr. The assertion bound is
//! generous (2 s, hard) so cold-cache CI hosts don't false-fail; the
//! surfaced 500 ms comparison is the spec-level pass/fail signal
//! (SC-302). Hardware-dependent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Publisher poll cadence we drive the service at — lower than the
/// production default (250 ms) so the test wall-clock stays tight
/// under SC-302's 500 ms budget. Matches buffer_open_bootstrap.rs.
const POLL_INTERVAL: &str = "100ms";
const SC302_BUDGET: Duration = Duration::from_millis(500);
/// Hard bound on waiting for either transition — if we don't see the
/// dirty flip in 2 s, something is structurally broken (not merely
/// slow). The SC-302 timing is reported separately for the operator.
const TRANSITION_HARD_BUDGET: Duration = Duration::from_millis(2_000);

#[tokio::test]
async fn buffer_external_mutation_sc302_dirty_flip() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    let original = b"hello buffer\n";
    std::fs::write(&fixture_path, original).unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical),
    ));

    // Wait until bootstrap's `buffer/dirty=false` lands — that's the
    // quiescent pre-mutation state the SC-302 budget measures FROM.
    wait_for_dirty(&mut observer, false, TRANSITION_HARD_BUDGET)
        .await
        .expect("bootstrap buffer/dirty=false");

    // Flip 1: mutate on disk; measure false → true.
    std::fs::write(&canonical, b"mutated\n").unwrap();
    let dirty_start = Instant::now();
    wait_for_dirty(&mut observer, true, TRANSITION_HARD_BUDGET)
        .await
        .expect("buffer/dirty=true after mutation");
    let dirty_elapsed = dirty_start.elapsed();
    eprintln!("[sc-302] buffer/dirty false→true in {dirty_elapsed:?} (budget {SC302_BUDGET:?})");

    // Flip 2: revert on disk; measure true → false.
    std::fs::write(&canonical, original).unwrap();
    let clean_start = Instant::now();
    wait_for_dirty(&mut observer, false, TRANSITION_HARD_BUDGET)
        .await
        .expect("buffer/dirty=false after revert");
    let clean_elapsed = clean_start.elapsed();
    eprintln!("[sc-302] buffer/dirty true→false in {clean_elapsed:?} (budget {SC302_BUDGET:?})");

    assert!(
        dirty_elapsed <= TRANSITION_HARD_BUDGET,
        "false→true flip exceeded hard budget {TRANSITION_HARD_BUDGET:?} ({dirty_elapsed:?})"
    );
    assert!(
        clean_elapsed <= TRANSITION_HARD_BUDGET,
        "true→false flip exceeded hard budget {TRANSITION_HARD_BUDGET:?} ({clean_elapsed:?})"
    );
}

async fn wait_for_dirty(
    observer: &mut Client,
    target: bool,
    budget: Duration,
) -> Result<Fact, Duration> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if buffer_service_id(&fact) != Some("weaver-buffers") {
            continue;
        }
        if fact.key.attribute != "buffer/dirty" {
            continue;
        }
        if let FactValue::Bool(b) = fact.value
            && b == target
        {
            return Ok(fact);
        }
    }
    Err(budget)
}

fn buffer_service_id(fact: &Fact) -> Option<&str> {
    match &fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => Some(service_id.as_str()),
        _ => None,
    }
}

// ---- subprocess helpers (mirror buffer_open_bootstrap.rs / git_watcher.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-buf-mut-e2e-{pid}-{tick}"));
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

fn spawn_buffer_service(socket: &Path, paths: &[PathBuf]) -> std::process::Child {
    let bin = build_buffer_service_binary();
    let mut cmd = Command::new(&bin);
    for p in paths {
        cmd.arg(p);
    }
    cmd.arg("--socket")
        .arg(socket)
        .arg(format!("--poll-interval={POLL_INTERVAL}"))
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
    std::env::temp_dir().join(format!("weaver-buf-mut-e2e-{pid}-{tick}.sock"))
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
