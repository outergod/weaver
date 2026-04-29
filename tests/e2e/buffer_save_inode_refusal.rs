//! T024 — slice-005 SC-502 + SC-503 e2e: external mutation refuses save.
//!
//! Six-process scenario per `specs/005-buffer-save/quickstart.md
//! §Scenarios 4 + 5`: core + git-watcher + weaver-buffers (with
//! stderr capture under `RUST_LOG=weaver_buffers=info`) + AllFacts
//! observer + a short-lived `weaver edit` invocation + a short-lived
//! `weaver save` invocation. Between the edit and the save, an
//! external process mutates the on-disk file.
//!
//! Three scenarios in this file map onto the dispatcher's two
//! refusal outcomes per `spec.md §102.4` and FR-016/FR-017 — the
//! split is finer-grained than SC-502's narrative wording:
//!
//!   * `mv` / `rm` (path is gone) → `SaveOutcome::PathMissing` →
//!     `WEAVER-SAVE-006`.
//!   * atomic-replace (path stays, inode differs) →
//!     `SaveOutcome::InodeMismatch` → `WEAVER-SAVE-005`.
//!
//! 1. [`external_rename_away_between_open_and_save_fires_save_006`]
//!    (SC-503 partial) — `mv <PATH> <PATH>.bak`; assert
//!    `WEAVER-SAVE-006`, original content preserved at `<PATH>.bak`,
//!    nothing at `<PATH>`, `buffer/dirty=true` not flipped.
//!
//! 2. [`external_delete_between_open_and_save_fires_save_006`]
//!    (SC-503) — `rm <PATH>`; assert `WEAVER-SAVE-006`, no recreation,
//!    `buffer/dirty=true` not flipped.
//!
//! 3. [`external_atomic_replace_fires_save_005`] (SC-502) — replace
//!    `<PATH>` with a different file via rename; assert
//!    `WEAVER-SAVE-005` with `expected_inode != actual_inode`,
//!    externally-written content preserved (no clobber),
//!    `buffer/dirty=true` not flipped.
//!
//! Tests 1 and 2 dispatch via `weaver save <entity-as-u64>` because
//! the path-form resolver canonicalises (`std::fs::canonicalize`)
//! before dispatch — a missing path would short-circuit at the CLI
//! with WEAVER-101 and never reach the dispatcher's R4 step. Test 3
//! uses path-form because the path still exists post-replace.

use std::io::Read as _;
use std::os::unix::fs::MetadataExt;
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
/// Watch window for the absence-of-`dirty=false` check after a
/// dispatched save that is expected to refuse. Long enough to outpace
/// the publisher's 100 ms poll-tick + bus round-trip; short enough
/// that the test wall-clock stays bounded under CI cold-cache.
const REFUSAL_WATCH_WINDOW: Duration = Duration::from_millis(750);

#[tokio::test]
async fn external_rename_away_between_open_and_save_fires_save_006() {
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
    let fixture_path = fixture_dir.join("rename-away.txt");
    let pre_edit_bytes: &[u8] = b"original content\n";
    std::fs::write(&fixture_path, pre_edit_bytes).unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize");
    let entity = buffer_entity_ref(&canonical);
    let backup_path = fixture_dir.join("rename-away.txt.bak");

    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");
    let buffers_child = spawn_buffer_service_with_log(&socket, &canonical, stderr_file, "info");
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

    // Edit drives buffer/dirty=true.
    let weaver = build_weaver_binary();
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical.to_str().unwrap(),
            "0:0-0:0",
            "DIRTY ",
        ],
    );
    assert!(
        edit_output.status.success(),
        "weaver edit must exit 0 (stderr={})",
        String::from_utf8_lossy(&edit_output.stderr),
    );
    drain_until_dirty(&mut observer, true).await;

    // External rename. The captured inode at open-time is preserved
    // at .bak (rename(2) keeps the inode); the original path no
    // longer exists.
    std::fs::rename(&canonical, &backup_path).expect("external rename");
    assert!(
        !canonical.exists(),
        "post-mv: <PATH> must not exist at {}",
        canonical.display()
    );

    // Dispatch via u64-form to bypass CLI canonicalisation (the path
    // is gone). Hits the dispatcher's R4 stat → PathMissing → -006.
    let entity_arg = entity.as_u64().to_string();
    let save_output = run_weaver(
        &weaver,
        &["--socket", socket.to_str().unwrap(), "save", &entity_arg],
    );
    assert!(
        save_output.status.success(),
        "weaver save must exit 0 (refusal is logged service-side, not CLI; status={:?}, stderr={})",
        save_output.status,
        String::from_utf8_lossy(&save_output.stderr),
    );

    // Expect NO buffer/dirty=false re-emission — refusal is silent
    // on the wire (FR-016/-017).
    assert!(
        !saw_dirty_false_within(&mut observer, REFUSAL_WATCH_WINDOW).await,
        "external rename + save MUST NOT trigger buffer/dirty=false re-emission",
    );

    // Pre-save content preserved at the renamed path.
    let backup_bytes = std::fs::read(&backup_path).expect("read .bak");
    assert_eq!(
        backup_bytes, pre_edit_bytes,
        "renamed file must keep pre-edit content (the buffer's edit was in-memory only; \
         no atomic-write happened against this inode)",
    );
    assert!(
        !canonical.exists(),
        "<PATH> must remain absent (no recreation by the refused save)"
    );

    let stderr_buf = stop_and_read_stderr(buffers_pid, &stderr_path);
    let entity_field = format!("entity={}", entity.as_u64());
    assert!(
        stderr_buf.contains("WEAVER-SAVE-006"),
        "expected WEAVER-SAVE-006 in weaver-buffers stderr; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains("path missing on save"),
        "expected `path missing on save` log line; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains(&entity_field),
        "expected `{entity_field}` in WEAVER-SAVE-006 record; got:\n{stderr_buf}",
    );
}

#[tokio::test]
async fn external_delete_between_open_and_save_fires_save_006() {
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
    let fixture_path = fixture_dir.join("deleted.txt");
    std::fs::write(&fixture_path, b"to be deleted\n").unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize");
    let entity = buffer_entity_ref(&canonical);

    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");
    let buffers_child = spawn_buffer_service_with_log(&socket, &canonical, stderr_file, "info");
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
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical.to_str().unwrap(),
            "0:0-0:0",
            "Z",
        ],
    );
    assert!(edit_output.status.success(), "weaver edit must exit 0");
    drain_until_dirty(&mut observer, true).await;

    std::fs::remove_file(&canonical).expect("external rm");
    assert!(!canonical.exists(), "<PATH> must be gone post-rm");

    let entity_arg = entity.as_u64().to_string();
    let save_output = run_weaver(
        &weaver,
        &["--socket", socket.to_str().unwrap(), "save", &entity_arg],
    );
    assert!(
        save_output.status.success(),
        "weaver save must exit 0 (refusal is logged service-side); stderr={}",
        String::from_utf8_lossy(&save_output.stderr),
    );

    assert!(
        !saw_dirty_false_within(&mut observer, REFUSAL_WATCH_WINDOW).await,
        "external delete + save MUST NOT trigger buffer/dirty=false re-emission",
    );

    assert!(
        !canonical.exists(),
        "<PATH> must remain absent (the refused save did not recreate it)",
    );

    let stderr_buf = stop_and_read_stderr(buffers_pid, &stderr_path);
    let entity_field = format!("entity={}", entity.as_u64());
    assert!(
        stderr_buf.contains("WEAVER-SAVE-006"),
        "expected WEAVER-SAVE-006 in weaver-buffers stderr; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains(&entity_field),
        "expected `{entity_field}` in WEAVER-SAVE-006 record; got:\n{stderr_buf}",
    );
}

#[tokio::test]
async fn external_atomic_replace_fires_save_005() {
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
    let fixture_path = fixture_dir.join("atomic-replace.txt");
    let pre_edit_bytes: &[u8] = b"buffer-side original\n";
    std::fs::write(&fixture_path, pre_edit_bytes).unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize");
    let entity = buffer_entity_ref(&canonical);
    let expected_inode = std::fs::metadata(&canonical)
        .expect("stat fixture pre-open")
        .ino();

    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");
    let buffers_child = spawn_buffer_service_with_log(&socket, &canonical, stderr_file, "info");
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
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical.to_str().unwrap(),
            "0:0-0:0",
            "DIRTY ",
        ],
    );
    assert!(edit_output.status.success(), "weaver edit must exit 0");
    drain_until_dirty(&mut observer, true).await;

    // External atomic-replace: write a sibling file in the same
    // directory, then rename(2) it over the canonical path. rename(2)
    // preserves the source inode, which is fresh and therefore
    // distinct from the open-time captured inode.
    let replacement_path = fixture_dir.join("atomic-replace.txt.new");
    let externally_written: &[u8] = b"externally-written content\n";
    std::fs::write(&replacement_path, externally_written).expect("write replacement");
    let actual_inode = std::fs::metadata(&replacement_path)
        .expect("stat replacement pre-rename")
        .ino();
    assert_ne!(
        expected_inode, actual_inode,
        "test setup invariant: replacement must have a different inode",
    );
    std::fs::rename(&replacement_path, &canonical).expect("atomic-replace rename");

    // Path still exists after replace; path-form is fine here.
    let save_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "save",
            canonical.to_str().unwrap(),
        ],
    );
    assert!(
        save_output.status.success(),
        "weaver save must exit 0 (refusal is logged service-side); stderr={}",
        String::from_utf8_lossy(&save_output.stderr),
    );

    assert!(
        !saw_dirty_false_within(&mut observer, REFUSAL_WATCH_WINDOW).await,
        "atomic-replace + save MUST NOT trigger buffer/dirty=false re-emission",
    );

    // No-clobber invariant: the externally-written content survives.
    let on_disk = std::fs::read(&canonical).expect("re-read canonical");
    assert_eq!(
        on_disk, externally_written,
        "atomic-rename invariant under refusal: externally-written content must survive (no clobber)",
    );

    let stderr_buf = stop_and_read_stderr(buffers_pid, &stderr_path);
    let entity_field = format!("entity={}", entity.as_u64());
    let expected_field = format!("expected_inode={expected_inode}");
    let actual_field = format!("actual_inode={actual_inode}");
    assert!(
        stderr_buf.contains("WEAVER-SAVE-005"),
        "expected WEAVER-SAVE-005 in weaver-buffers stderr; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains("path/inode mismatch on save"),
        "expected `path/inode mismatch on save` log line; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains(&entity_field),
        "expected `{entity_field}` in WEAVER-SAVE-005 record; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains(&expected_field) && stderr_buf.contains(&actual_field),
        "expected `{expected_field}` + `{actual_field}` fields in WEAVER-SAVE-005 record; got:\n{stderr_buf}",
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_save_dirty.rs / buffer_save_clean_noop.rs)
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

/// Watch for any `buffer/dirty=false` fact within the window. Used to
/// pin the silent-refusal contract: refused saves MUST NOT publish
/// `buffer/dirty=false` (FR-016/-017). Returns `true` iff one was
/// observed (test failure).
async fn saw_dirty_false_within(observer: &mut Client, window: Duration) -> bool {
    let deadline = Instant::now() + window;
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

/// SIGTERM the buffer-service so tracing's buffered writes flush,
/// then read the captured stderr file.
fn stop_and_read_stderr(buffers_pid: u32, stderr_path: &Path) -> String {
    let _ = nix_signal(buffers_pid, libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(300));
    let mut buf = String::new();
    let mut f = std::fs::File::open(stderr_path).expect("open stderr capture");
    f.read_to_string(&mut buf).expect("read stderr");
    buf
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
    let p = std::env::temp_dir().join(format!("weaver-save-refusal-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-save-refusal-e2e-{pid}-{tick}.sock"))
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
