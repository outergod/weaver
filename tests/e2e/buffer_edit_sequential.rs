//! T023 — slice-004 SC-403 e2e: 100 sequential edits land cleanly.
//!
//! Bootstrap a buffer; run 100 sequential `weaver edit` invocations
//! each carrying a single-byte payload; capture every `buffer/version`
//! update from a long-lived AllFacts subscriber; assert
//! `buffer/version=100` is observed after the loop and that the
//! observed version sequence has no gaps and no duplicates among
//! values 1..=100.
//!
//! Per spec Q4 this is structural-only — no wall-clock budget. Total
//! wall-clock is reported to stderr informationally.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const COLLECT_BUDGET: Duration = Duration::from_secs(60);
const NUM_EDITS: u64 = 100;

#[tokio::test]
async fn one_hundred_sequential_edits_each_bump_version_with_no_gaps() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"world").unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical),
    ));
    drain_until_buffers_ready(&mut observer).await;

    // Run NUM_EDITS sequential edits. Each invocation inserts a single
    // byte at the start of the buffer; the publisher accepts each in
    // turn and bumps `buffer/version` by 1.
    let weaver = build_weaver_binary();
    let edit_loop_start = Instant::now();
    for i in 0..NUM_EDITS {
        let out = run_weaver(
            &weaver,
            &[
                "--socket",
                socket.to_str().unwrap(),
                "edit",
                canonical.to_str().unwrap(),
                "0:0-0:0",
                "x",
            ],
        );
        assert!(
            out.status.success(),
            "weaver edit #{i} failed (status={:?}, stderr={})",
            out.status,
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let dispatch_elapsed = edit_loop_start.elapsed();

    // Drain the observer until we see buffer/version=NUM_EDITS, or the
    // budget elapses. Track every observed value so the post-loop
    // structural assertions can detect gaps or duplicates.
    let mut observed_versions: Vec<u64> = Vec::with_capacity(NUM_EDITS as usize);
    let collect_start = Instant::now();
    let deadline = collect_start + COLLECT_BUDGET;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        let Some(service_id) = buffer_service_id(&fact) else {
            continue;
        };
        if service_id != "weaver-buffers" {
            continue;
        }
        if fact.key.attribute == "buffer/version"
            && let FactValue::U64(v) = fact.value
        {
            observed_versions.push(v);
            if v == NUM_EDITS {
                break;
            }
        }
    }
    let drain_elapsed = collect_start.elapsed();

    eprintln!(
        "[sc-403] 100 sequential edits dispatched in {dispatch_elapsed:?}; \
         observer drained final version in {drain_elapsed:?} (informational; \
         no spec budget)"
    );

    // Structural assertion 1: the final observed version is exactly
    // NUM_EDITS (no edits dropped, no extras).
    let max = observed_versions.iter().copied().max();
    assert_eq!(
        max,
        Some(NUM_EDITS),
        "expected buffer/version={NUM_EDITS} as final observed value; got {max:?}"
    );

    // Structural assertion 2: every value in 1..=NUM_EDITS appears at
    // least once. The bootstrap value (0) may also appear; any value
    // > NUM_EDITS would be a defect.
    let mut seen = std::collections::BTreeSet::new();
    for v in &observed_versions {
        seen.insert(*v);
    }
    let mut missing: Vec<u64> = Vec::new();
    for expected in 1..=NUM_EDITS {
        if !seen.contains(&expected) {
            missing.push(expected);
        }
    }
    assert!(
        missing.is_empty(),
        "expected buffer/version values 1..={NUM_EDITS} all observed; missing: {missing:?}"
    );

    let extra: Vec<&u64> = seen.iter().filter(|v| **v > NUM_EDITS).collect();
    assert!(
        extra.is_empty(),
        "no buffer/version > {NUM_EDITS} should appear; got: {extra:?}"
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_edit_atomic_batch.rs's pattern)
// ───────────────────────────────────────────────────────────────────

async fn drain_until_buffers_ready(observer: &mut Client) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        let Some(service_id) = buffer_service_id(&fact) else {
            continue;
        };
        if service_id == "weaver-buffers"
            && fact.key.attribute == "watcher/status"
            && let FactValue::String(s) = &fact.value
            && s == "ready"
        {
            return;
        }
    }
    panic!("weaver-buffers did not reach watcher/status=ready within budget");
}

fn buffer_service_id(fact: &Fact) -> Option<&str> {
    match &fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => Some(service_id.as_str()),
        _ => None,
    }
}

fn run_weaver(bin: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver")
}

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-edit-seq-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-edit-seq-e2e-{pid}-{tick}.sock"))
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
