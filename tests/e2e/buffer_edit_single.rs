//! T015 — slice-004 SC-401 e2e: single-edit dispatch.
//!
//! Five-process scenario per `specs/004-buffer-edit/research.md §9`:
//! core + git-watcher + weaver-buffers + AllFacts observer + a
//! short-lived `weaver edit` invocation.
//!
//! Two scenarios in this file:
//!
//! 1. [`single_edit_lands_with_version_bump_and_dirty_flip`] —
//!    bootstrap a buffer at `buffer/version=0`; dispatch
//!    `weaver edit <PATH> 0:0-0:0 "PREFIX "`; assert the observer
//!    sees `buffer/version=1`, `buffer/byte-size` advanced by 7, and
//!    `buffer/dirty=true` within a structural collection budget.
//!    Reports the observed dispatch-to-`buffer/version=1` wall-clock
//!    to stderr (informational SC-401 measurement; the operator
//!    judges the ≤500 ms budget via T028).
//!
//! 2. [`buffer_not_opened_returns_exit_1`] — with NO `weaver-buffers`
//!    running (only core + observer), invoke `weaver edit
//!    /tmp/<unique> 0:0-0:0 "x"`. Assert exit code 1 + stderr
//!    contains `WEAVER-EDIT-001` + `"buffer not opened"`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// SC-401 spec budget — reported for operator judgement, not used as
/// the hard assertion bound (see EDIT_COLLECT_BUDGET).
const SC401_BUDGET: Duration = Duration::from_millis(500);
/// Hard collection window used to bound the test wall-clock. Slack
/// above SC-401 accommodates cold-cache binary spawns in CI without
/// masking real regressions.
const EDIT_COLLECT_BUDGET: Duration = Duration::from_millis(5_000);

#[tokio::test]
async fn single_edit_lands_with_version_bump_and_dirty_flip() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_git_watcher_binary();
    build_buffer_service_binary();

    // Empty git repo for the git-watcher sidecar (5-process compliance:
    // mirror buffer_open_bootstrap.rs's "other service already on the
    // bus" deployment shape).
    let repo_dir = tempdir();
    git(&repo_dir, &["init", "-b", "main", "-q"]);
    git(&repo_dir, &["config", "user.email", "e2e@test.invalid"]);
    git(&repo_dir, &["config", "user.name", "e2e"]);
    std::fs::write(repo_dir.join("seed.txt"), "seed").unwrap();
    git(&repo_dir, &["add", "seed.txt"]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);
    let _watcher = ChildGuard::new(spawn_git_watcher(&socket, &repo_dir));

    // Subscribe BEFORE the buffer service launches so we capture the
    // bootstrap stream end-to-end and the post-edit re-emit burst.
    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // Fixture file the buffer service will open + the CLI will edit.
    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    let fixture_bytes = b"world";
    std::fs::write(&fixture_path, fixture_bytes).unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));

    // Drain the bootstrap burst until we see watcher/status=ready
    // (signals the buffer service is fully bootstrapped + listening).
    drain_until_buffers_ready(&mut observer).await;

    // Dispatch the edit. Stopwatch starts here to give the operator a
    // dispatch-to-version-bump wall-clock for SC-401 judgement.
    let inserted = "PREFIX ";
    let weaver = build_weaver_binary();
    let dispatch_start = Instant::now();
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            inserted,
        ],
    );
    assert!(
        edit_output.status.success(),
        "weaver edit must exit 0 (status={:?}, stderr={})",
        edit_output.status,
        String::from_utf8_lossy(&edit_output.stderr),
    );

    // Collect the post-edit re-emission burst from weaver-buffers.
    // The publisher emits buffer/byte-size, buffer/version, buffer/dirty
    // in that order, all causally parented to the BufferEdit event.id.
    let expected_byte_size = (fixture_bytes.len() + inserted.len()) as u64;
    let mut got_byte_size = false;
    let mut got_version_one = false;
    let mut got_dirty_true = false;
    let mut version_one_at: Option<Duration> = None;

    let deadline = dispatch_start + EDIT_COLLECT_BUDGET;
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
        match fact.key.attribute.as_str() {
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fact.value
                    && n == expected_byte_size
                {
                    got_byte_size = true;
                }
            }
            "buffer/version" => {
                if let FactValue::U64(v) = fact.value
                    && v == 1
                {
                    got_version_one = true;
                    version_one_at = Some(dispatch_start.elapsed());
                }
            }
            "buffer/dirty" => {
                if let FactValue::Bool(true) = fact.value {
                    got_dirty_true = true;
                }
            }
            _ => {}
        }
        if got_byte_size && got_version_one && got_dirty_true {
            break;
        }
    }

    let observed = version_one_at.unwrap_or_else(|| dispatch_start.elapsed());
    eprintln!(
        "[sc-401] weaver edit → buffer/version=1 in {observed:?} (budget {SC401_BUDGET:?}; \
         operator judges via T028)"
    );

    assert!(
        got_byte_size,
        "missing buffer/byte-size={expected_byte_size} in re-emission burst",
    );
    assert!(
        got_version_one,
        "missing buffer/version=1 in re-emission burst"
    );
    assert!(
        got_dirty_true,
        "missing buffer/dirty=true in re-emission burst (in-memory diverged from disk)"
    );
    assert!(
        observed <= EDIT_COLLECT_BUDGET,
        "edit exceeded hard collection budget {EDIT_COLLECT_BUDGET:?} (observed {observed:?})"
    );

    // Disk content invariant (FR-013): no save-to-disk this slice.
    let on_disk = std::fs::read(&canonical_fixture).expect("re-read fixture");
    assert_eq!(
        on_disk, fixture_bytes,
        "FR-013: edits MUST NOT touch the on-disk content this slice"
    );
}

#[tokio::test]
async fn buffer_not_opened_returns_exit_1() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;
    build_weaver_binary();

    // No weaver-buffers — the inspect-lookup for buffer/version
    // returns FactNotFound and the CLI exits 1 with WEAVER-EDIT-001.
    // Use a real existing file so canonicalize() succeeds; the
    // buffer-not-opened path triggers on the inspect-lookup, not on
    // path validity.
    let fixture_dir = tempdir();
    let path = fixture_dir.join("never-opened.txt");
    std::fs::write(&path, b"placeholder\n").expect("write fixture");
    let canonical = std::fs::canonicalize(&path).expect("canonicalize");

    let weaver = build_weaver_binary();
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "exit must be 1 for buffer-not-opened (got {:?}, stderr={})",
        out.status,
        stderr,
    );
    assert!(
        stderr.contains("WEAVER-EDIT-001"),
        "stderr must contain WEAVER-EDIT-001, got: {stderr}"
    );
    assert!(
        stderr.contains("buffer not opened"),
        "stderr must contain `buffer not opened`, got: {stderr}"
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_open_bootstrap.rs's pattern)
// ───────────────────────────────────────────────────────────────────

async fn drain_until_buffers_ready(observer: &mut Client) {
    let deadline = Instant::now() + EDIT_COLLECT_BUDGET;
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
    let p = std::env::temp_dir().join(format!("weaver-edit-e2e-{pid}-{tick}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success());
    bin_path("weaver")
}

fn build_git_watcher_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args([
            "build",
            "--quiet",
            "-p",
            "weaver-git-watcher",
            "--bin",
            "weaver-git-watcher",
        ])
        .status()
        .expect("cargo build weaver-git-watcher");
    assert!(status.success());
    bin_path("weaver-git-watcher")
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

fn spawn_git_watcher(socket: &Path, repo: &Path) -> std::process::Child {
    let bin = build_git_watcher_binary();
    Command::new(&bin)
        .arg(repo)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver-git-watcher")
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
    std::env::temp_dir().join(format!("weaver-edit-e2e-{pid}-{tick}.sock"))
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
