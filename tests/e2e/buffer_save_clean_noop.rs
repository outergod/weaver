//! T023 — slice-005 SC-507 + SC-505 partial e2e: clean-save no-op.
//!
//! Six-process scenario per `specs/005-buffer-save/quickstart.md
//! §Scenario 7`: bootstrap a buffer, drive `buffer/dirty=true` via
//! `weaver edit`, then `weaver save` to flip it to `false`. A second
//! `weaver save` against the now-clean buffer is the structural focus.
//!
//! Two scenarios in this file:
//!
//! 1. [`clean_save_emits_save_007_and_idempotent_dirty_reemit`]
//!    (SC-507) — the second save emits `WEAVER-SAVE-007` at info on
//!    weaver-buffers stderr, idempotently re-asserts
//!    `buffer/dirty = false`, and preserves the file's mtime
//!    byte-for-byte (no disk I/O).
//!
//! 2. [`clean_save_inspect_why_walks_to_latest_save`] (SC-505 partial)
//!    — `weaver inspect --why <entity>:buffer/dirty -o json` walks
//!    back to a `BufferSave` event (not the original `BufferOpen` /
//!    `BufferEdit`); `event.provenance.source.type == "user"`.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::buffer_entity::buffer_entity_ref;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const COLLECT_BUDGET: Duration = Duration::from_millis(5_000);
/// Watch window for the absence-of-mtime-change check in scenario 1.
/// 1.2 s comfortably exceeds 1 s mtime granularity on most
/// filesystems; the post-save mtime equality assertion is checked
/// AFTER this window elapses.
const MTIME_PRESERVATION_WINDOW: Duration = Duration::from_millis(1_200);

#[tokio::test]
async fn clean_save_emits_save_007_and_idempotent_dirty_reemit() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_git_watcher_binary();
    build_buffer_service_binary();

    let repo_dir = tempdir();
    git(&repo_dir, &["init", "-b", "main", "-q"]);
    git(&repo_dir, &["config", "user.email", "e2e@test.invalid"]);
    git(&repo_dir, &["config", "user.name", "e2e"]);
    std::fs::write(repo_dir.join("seed.txt"), "seed").unwrap();
    git(&repo_dir, &["add", "seed.txt"]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);
    let _watcher = ChildGuard::new(spawn_git_watcher(&socket, &repo_dir));

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("clean.txt");
    std::fs::write(&fixture_path, b"hello").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize");

    // Capture weaver-buffers stderr under RUST_LOG=info so we can
    // grep for the WEAVER-SAVE-007 record after the service exits.
    // info-level emission means we don't need debug filtering here.
    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");
    let buffers_child =
        spawn_buffer_service_with_log(&socket, &canonical_fixture, stderr_file, "info");
    let buffers_pid = buffers_child.id();
    let _buffers = ChildGuard::new(buffers_child);

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");
    drain_until_buffers_ready(&mut observer).await;

    let weaver = build_weaver_binary();

    // Drive dirty=true via an edit.
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            "X",
        ],
    );
    assert!(edit_output.status.success(), "weaver edit must exit 0");
    drain_until_dirty(&mut observer, true).await;

    // First save → flip dirty back to false. After this the buffer
    // is clean and the file's mtime is whatever the first save's
    // atomic-rename left.
    let save1 = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical_fixture.to_str().unwrap(),
        ],
    );
    assert!(save1.status.success(), "first weaver save must exit 0");
    drain_until_dirty(&mut observer, false).await;
    let mtime_before_second_save = std::fs::metadata(&canonical_fixture)
        .expect("stat post-first-save")
        .modified()
        .expect("mtime supported");

    // Sleep past mtime granularity so a hypothetical disk write
    // would produce a measurably-different mtime.
    sleep(MTIME_PRESERVATION_WINDOW).await;

    // Second save against a now-clean buffer — the SC-507 path. The
    // dispatcher's R3 branch fires, no disk I/O happens, but the
    // buffer/dirty=false fact is re-asserted idempotently.
    let save2 = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical_fixture.to_str().unwrap(),
        ],
    );
    assert!(save2.status.success(), "second weaver save must exit 0");

    // Observe the post-clean-save buffer/dirty=false re-emission. We
    // already saw one after the first save, so this is the third
    // one in the trace overall (bootstrap → first-save → second-save).
    let saw_dirty_false_again = wait_for_dirty_false(&mut observer).await;
    assert!(
        saw_dirty_false_again,
        "expected idempotent buffer/dirty=false re-emission after clean save"
    );

    // mtime preservation invariant: clean-save performs no disk I/O.
    let mtime_after_second_save = std::fs::metadata(&canonical_fixture)
        .expect("stat post-second-save")
        .modified()
        .expect("mtime supported");
    assert_eq!(
        mtime_before_second_save, mtime_after_second_save,
        "SC-507: mtime must be preserved across a clean save (no disk I/O)"
    );

    // Drain stderr to verify WEAVER-SAVE-007 emission. The stop
    // ensures tracing's buffered writes flush before we read.
    let _ = nix_signal(buffers_pid, libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(300));
    let mut stderr_buf = String::new();
    let mut f = std::fs::File::open(&stderr_path).expect("open stderr capture");
    f.read_to_string(&mut stderr_buf).expect("read stderr");
    assert!(
        stderr_buf.contains("WEAVER-SAVE-007"),
        "expected WEAVER-SAVE-007 in weaver-buffers stderr; got:\n{stderr_buf}"
    );
    assert!(
        stderr_buf.contains("nothing to save"),
        "expected `nothing to save` in WEAVER-SAVE-007 record; got:\n{stderr_buf}"
    );
}

#[tokio::test]
async fn clean_save_inspect_why_walks_to_latest_save() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_git_watcher_binary();
    build_buffer_service_binary();

    let repo_dir = tempdir();
    git(&repo_dir, &["init", "-b", "main", "-q"]);
    git(&repo_dir, &["config", "user.email", "e2e@test.invalid"]);
    git(&repo_dir, &["config", "user.name", "e2e"]);
    std::fs::write(repo_dir.join("seed.txt"), "seed").unwrap();
    git(&repo_dir, &["add", "seed.txt"]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);
    let _watcher = ChildGuard::new(spawn_git_watcher(&socket, &repo_dir));

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("inspect.txt");
    std::fs::write(&fixture_path, b"hello").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");
    drain_until_buffers_ready(&mut observer).await;

    let weaver = build_weaver_binary();

    // Edit → first save → second save (clean). The latest BufferSave
    // is what the walkback should land on.
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            "X",
        ],
    );
    assert!(edit_output.status.success(), "weaver edit must exit 0");
    drain_until_dirty(&mut observer, true).await;

    let save1 = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical_fixture.to_str().unwrap(),
        ],
    );
    assert!(save1.status.success(), "first weaver save must exit 0");
    drain_until_dirty(&mut observer, false).await;

    let save2 = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical_fixture.to_str().unwrap(),
        ],
    );
    assert!(save2.status.success(), "second weaver save must exit 0");
    let saw_dirty_false_again = wait_for_dirty_false(&mut observer).await;
    assert!(
        saw_dirty_false_again,
        "second save's dirty=false re-emission"
    );

    let buffer_entity = buffer_entity_ref(&canonical_fixture);
    let inspect_key = format!("{}:buffer/dirty", buffer_entity.as_u64());
    let inspect = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "-o",
            "json",
            "inspect",
            &inspect_key,
            "--why",
        ],
    );
    assert!(
        inspect.status.success(),
        "weaver inspect --why must exit 0 (status={:?}, stderr={})",
        inspect.status,
        String::from_utf8_lossy(&inspect.stderr),
    );
    let stdout = String::from_utf8(inspect.stdout).expect("utf-8 output");
    let v: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("non-JSON output: {e}\n{stdout}"));

    let event = v.get("event").expect("event block present");
    assert_eq!(
        event.get("payload_type").and_then(|s| s.as_str()),
        Some("buffer-save"),
        "walkback target's payload_type must be buffer-save (latest re-assertion)",
    );
    assert_eq!(
        event.get("target").and_then(|n| n.as_u64()),
        Some(buffer_entity.as_u64()),
        "event.target must match the buffer entity",
    );

    let provenance = event.get("provenance").expect("event.provenance present");
    let source = provenance.get("source").expect("provenance.source present");
    let source_type = source
        .get("type")
        .and_then(|s| s.as_str())
        .expect("provenance.source.type present");
    assert_eq!(
        source_type, "user",
        "SC-505 partial: BufferSave emitter MUST be ActorIdentity::User; got source={source}",
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_save_dirty.rs's pattern)
// ───────────────────────────────────────────────────────────────────

async fn drain_until_buffers_ready(observer: &mut Client) {
    let deadline = Instant::now() + COLLECT_BUDGET;
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

async fn drain_until_dirty(observer: &mut Client, want: bool) {
    let deadline = Instant::now() + COLLECT_BUDGET;
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
        if fact.key.attribute == "buffer/dirty"
            && let FactValue::Bool(b) = fact.value
            && b == want
        {
            return;
        }
    }
    panic!("did not observe buffer/dirty={want} within budget");
}

async fn wait_for_dirty_false(observer: &mut Client) -> bool {
    let deadline = Instant::now() + COLLECT_BUDGET;
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
        if fact.key.attribute == "buffer/dirty"
            && let FactValue::Bool(false) = fact.value
        {
            return true;
        }
    }
    false
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
    let p = std::env::temp_dir().join(format!("weaver-save-clean-e2e-{pid}-{tick}"));
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

fn spawn_buffer_service_with_log(
    socket: &Path,
    path: &Path,
    stderr: std::fs::File,
    log_level: &str,
) -> std::process::Child {
    let bin = build_buffer_service_binary();
    Command::new(&bin)
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
        .env("RUST_LOG", format!("weaver_buffers={log_level}"))
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(stderr)
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
    std::env::temp_dir().join(format!("weaver-save-clean-e2e-{pid}-{tick}.sock"))
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
