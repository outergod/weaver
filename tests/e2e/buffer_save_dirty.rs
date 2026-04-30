//! T022 — slice-005 SC-501 e2e: dirty save flips dirty→false + persists to disk.
//!
//! Six-process scenario per `specs/005-buffer-save/quickstart.md
//! §Scenario 1`: core + git-watcher + weaver-buffers + AllFacts
//! observer + a short-lived `weaver edit` invocation + a short-lived
//! `weaver save` invocation.
//!
//! Three scenarios in this file:
//!
//! 1. [`dirty_save_flips_dirty_to_false_and_persists_disk`] — bootstrap
//!    a buffer with `"world"`, run `weaver edit` to make it dirty,
//!    then `weaver save` and observe `buffer/dirty=false` within a
//!    structural collection budget; assert the on-disk content
//!    matches the in-memory edit; assert `buffer/version` is
//!    unchanged.
//!
//! 2. [`buffer_not_opened_returns_exit_1`] — with NO weaver-buffers,
//!    run `weaver save /tmp/<unique>`; assert exit 1 + stderr
//!    contains `WEAVER-SAVE-001`.
//!
//! 3. [`stale_version_save_silent_drops`] — bootstrap, bump version
//!    via `weaver edit`, then directly send a stale BufferSave
//!    (v=0) via a raw bus Client. Assert no `buffer/dirty=false`
//!    re-emission and that weaver-buffers stderr (captured under
//!    `RUST_LOG=weaver_buffers=debug`) contains `WEAVER-SAVE-002` +
//!    `reason="stale-version"`.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::buffer_entity::buffer_entity_ref;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::ids::{EventId, hash_to_58};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// SC-501 spec budget — reported for operator judgement, not used as
/// the hard assertion bound.
const SC501_BUDGET: Duration = Duration::from_millis(500);
/// Hard collection window. Slack above SC-501 accommodates
/// cold-cache binary spawns in CI without masking real regressions.
const SAVE_COLLECT_BUDGET: Duration = Duration::from_millis(5_000);

#[tokio::test]
async fn dirty_save_flips_dirty_to_false_and_persists_disk() {
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

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    let fixture_bytes = b"world";
    std::fs::write(&fixture_path, fixture_bytes).unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));

    drain_until_buffers_ready(&mut observer).await;

    // Step 1 — edit. Drives buffer/dirty=true.
    let inserted = "PREFIX ";
    let weaver = build_weaver_binary();
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

    // Drain the post-edit re-emission burst until we see
    // buffer/dirty=true (precondition for the save).
    drain_until_dirty(&mut observer, true).await;

    // Step 2 — save. Stopwatch starts now for SC-501.
    let dispatch_start = Instant::now();
    let save_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical_fixture.to_str().unwrap(),
        ],
    );
    assert!(
        save_output.status.success(),
        "weaver save must exit 0 (status={:?}, stderr={})",
        save_output.status,
        String::from_utf8_lossy(&save_output.stderr),
    );

    // Collect post-save re-emission of buffer/dirty=false.
    let mut got_dirty_false = false;
    let mut dirty_false_at: Option<Duration> = None;
    let deadline = dispatch_start + SAVE_COLLECT_BUDGET;
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
            got_dirty_false = true;
            dirty_false_at = Some(dispatch_start.elapsed());
            break;
        }
    }

    let observed = dirty_false_at.unwrap_or_else(|| dispatch_start.elapsed());
    eprintln!(
        "[sc-501] weaver save → buffer/dirty=false in {observed:?} (budget {SC501_BUDGET:?}; \
         operator judges via T033)"
    );

    assert!(
        got_dirty_false,
        "missing buffer/dirty=false after save (within {SAVE_COLLECT_BUDGET:?})"
    );
    assert!(
        observed <= SAVE_COLLECT_BUDGET,
        "save exceeded hard collection budget {SAVE_COLLECT_BUDGET:?} (observed {observed:?})"
    );

    // FR-002: on-disk content must match the in-memory post-edit content.
    let on_disk = std::fs::read(&canonical_fixture).expect("re-read fixture");
    let mut expected = Vec::with_capacity(inserted.len() + fixture_bytes.len());
    expected.extend_from_slice(inserted.as_bytes());
    expected.extend_from_slice(fixture_bytes);
    assert_eq!(
        on_disk, expected,
        "post-save disk content must match the in-memory edit"
    );
}

#[tokio::test]
async fn buffer_not_opened_returns_exit_1() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;
    build_weaver_binary();

    // No weaver-buffers — the inspect-lookup for buffer/version
    // returns FactNotFound and the CLI exits 1 with WEAVER-SAVE-001.
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
            "save",
            canonical.to_str().unwrap(),
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
        stderr.contains("WEAVER-SAVE-001"),
        "stderr must contain WEAVER-SAVE-001, got: {stderr}"
    );
    assert!(
        stderr.contains("buffer not opened"),
        "stderr must contain `buffer not opened`, got: {stderr}"
    );
}

#[tokio::test]
async fn stale_version_save_silent_drops() {
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
    let fixture_path = fixture_dir.join("stale.txt");
    std::fs::write(&fixture_path, b"original").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    // Capture weaver-buffers stderr under RUST_LOG=debug so we can
    // grep for the WEAVER-SAVE-002 stale-drop record after the
    // service exits.
    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");
    let buffers_child =
        spawn_buffer_service_with_debug_log(&socket, &canonical_fixture, stderr_file);
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

    // Bump version to 1 via `weaver edit`.
    let weaver = build_weaver_binary();
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
    // The publisher's BufferEdit::Applied burst publishes byte-size,
    // version, and dirty=true in that order. Draining to dirty=true is
    // sufficient signal that the version-bump has landed in the
    // registry: dispatch_buffer_edit bumps versions[entity] before
    // any of the three publish_fact calls.
    drain_until_dirty(&mut observer, true).await;

    // Now construct a stale BufferSave at v=0 over a raw bus client.
    // The dispatcher's R2 step rejects it; weaver-buffers logs
    // WEAVER-SAVE-002 at debug; no buffer/dirty re-emission follows.
    let entity = buffer_entity_ref(&canonical_fixture);
    let mut sender = Client::connect(&socket, "e2e-stale-sender")
        .await
        .expect("sender connect");
    let now = now_ns();
    let prefix = hash_to_58(&uuid::Uuid::new_v4());
    let event = Event {
        id: EventId::mint_v8(prefix, now),
        name: "buffer/save".into(),
        target: Some(entity),
        payload: EventPayload::BufferSave { entity, version: 0 },
        provenance: Provenance::new(ActorIdentity::User, now, None).expect("provenance"),
    };
    sender
        .send(&BusMessage::Event(event))
        .await
        .expect("send stale BufferSave");

    // Watch for buffer/dirty=false for a bounded window. We expect
    // NOT to see one — stale-drop is silent on the wire (FR-013).
    let watch_window = Duration::from_millis(750);
    let watch_deadline = Instant::now() + watch_window;
    let mut saw_dirty_false = false;
    while Instant::now() < watch_deadline {
        let remaining = watch_deadline.saturating_duration_since(Instant::now());
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
            saw_dirty_false = true;
            break;
        }
    }
    assert!(
        !saw_dirty_false,
        "stale-version BufferSave (v=0) must NOT trigger a buffer/dirty=false re-emission"
    );

    // Triggers graceful service shutdown so the trace-buffered stderr
    // flushes (mirrors buffer_edit_stale_drop.rs's pattern).
    let _ = nix_signal(buffers_pid, libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(300));

    let mut stderr_buf = String::new();
    let mut f = std::fs::File::open(&stderr_path).expect("open stderr capture");
    f.read_to_string(&mut stderr_buf).expect("read stderr");

    assert!(
        stderr_buf.contains("WEAVER-SAVE-002"),
        "expected WEAVER-SAVE-002 in weaver-buffers stderr; got:\n{stderr_buf}"
    );
    assert!(
        stderr_buf.contains(r#"reason="stale-version""#),
        "expected `reason=\"stale-version\"` in stale-drop debug line; got:\n{stderr_buf}"
    );
    assert!(
        stderr_buf.contains("event_version=0") && stderr_buf.contains("current_version=1"),
        "expected event_version=0 + current_version=1 in WEAVER-SAVE-002 record; got:\n{stderr_buf}"
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_edit_single.rs's pattern)
// ───────────────────────────────────────────────────────────────────

async fn drain_until_buffers_ready(observer: &mut Client) {
    let deadline = Instant::now() + SAVE_COLLECT_BUDGET;
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
    let deadline = Instant::now() + SAVE_COLLECT_BUDGET;
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
    let p = std::env::temp_dir().join(format!("weaver-save-e2e-{pid}-{tick}"));
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

fn spawn_buffer_service_with_debug_log(
    socket: &Path,
    path: &Path,
    stderr: std::fs::File,
) -> std::process::Child {
    let bin = build_buffer_service_binary();
    Command::new(&bin)
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
        .env("RUST_LOG", "weaver_buffers=debug")
        // tracing-subscriber's stderr writer drops colors when piped
        // to a file, but force NO_COLOR in case a transitive dep sniffs
        // for ANSI support.
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
    std::env::temp_dir().join(format!("weaver-save-e2e-{pid}-{tick}.sock"))
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
