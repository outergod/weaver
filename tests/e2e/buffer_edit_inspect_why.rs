//! T016 — slice-004 SC-405 e2e: `weaver inspect --why` walks back
//! from `buffer/version` to the accepted `BufferEdit` event and
//! renders the emitter's `ActorIdentity::User` provenance.
//!
//! Five-process scenario per `research.md §9`: core + git-watcher +
//! weaver-buffers + AllFacts observer + two short-lived weaver
//! invocations (`weaver edit` + `weaver inspect --why`).
//!
//! Walkback assertions:
//!
//! - `fact.entity` matches the buffer's derived entity-id.
//! - `fact_inspection.asserting_kind == "service"` and
//!   `asserting_service == "weaver-buffers"` — proves the version
//!   bump was service-mediated (not behavior- or directly-published).
//! - `event.id == fact_inspection.source_event` — the chain walk
//!   uses that exact id as the EventInspectRequest's event_id.
//! - `event.payload_type == "buffer-edit"` — the source event was a
//!   BufferEdit, not an unrelated event with the same id (defensive).
//! - `event.target` matches the buffer entity (the BufferEdit
//!   targeted the right buffer).
//! - `event.provenance.source.type == "user"` — pins SC-405:
//!   `weaver edit` stamped `ActorIdentity::User` on dispatch
//!   (research §6 first-production-use).

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

#[tokio::test]
async fn inspect_why_walks_back_to_user_emitted_buffer_edit() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_git_watcher_binary();
    build_buffer_service_binary();

    // Empty git repo for the git-watcher sidecar (5-process compliance).
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

    // Fixture file the buffer service will open + the CLI will edit.
    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"world").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");
    let buffer_entity = buffer_entity_ref(&canonical_fixture);

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));

    drain_until_buffers_ready(&mut observer).await;

    // Dispatch a single edit.
    let weaver = build_weaver_binary();
    let edit_output = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            "PREFIX ",
        ],
    );
    assert!(
        edit_output.status.success(),
        "weaver edit must exit 0 (status={:?}, stderr={})",
        edit_output.status,
        String::from_utf8_lossy(&edit_output.stderr),
    );

    // Wait for buffer/version=1 to land (proves the edit was applied
    // before we walk back). The observer's already subscribed; we
    // drain until we see the U64(1) value.
    wait_for_buffer_version_one(&mut observer, buffer_entity).await;

    // Run weaver inspect --why on the entity:buffer/version key.
    let inspect_key = format!("{}:buffer/version", buffer_entity.as_u64());
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

    // ----- fact block -----
    let fact = v.get("fact").expect("fact block present");
    assert_eq!(
        fact.get("entity").and_then(|n| n.as_u64()),
        Some(buffer_entity.as_u64()),
        "fact.entity must match buffer entity-id",
    );
    assert_eq!(
        fact.get("attribute").and_then(|s| s.as_str()),
        Some("buffer/version"),
        "fact.attribute must be buffer/version",
    );

    // ----- fact_inspection block -----
    let inspection = v
        .get("fact_inspection")
        .expect("fact_inspection block present");
    assert_eq!(
        inspection.get("asserting_kind").and_then(|s| s.as_str()),
        Some("service"),
        "buffer/version must be service-asserted (by weaver-buffers); fact_inspection={inspection}",
    );
    assert_eq!(
        inspection.get("asserting_service").and_then(|s| s.as_str()),
        Some("weaver-buffers"),
        "asserting_service must be weaver-buffers",
    );
    let source_event = inspection
        .get("source_event")
        .and_then(|n| n.as_u64())
        .expect("source_event present and u64");

    // ----- event block (the walkback target) -----
    let event = v.get("event").expect("event block present");
    let event_id = event
        .get("id")
        .and_then(|n| n.as_u64())
        .expect("event.id present");
    assert_eq!(
        event_id, source_event,
        "INVARIANT: event.id must equal fact_inspection.source_event",
    );
    assert_eq!(
        event.get("payload_type").and_then(|s| s.as_str()),
        Some("buffer-edit"),
        "event.payload_type must be buffer-edit",
    );
    assert_eq!(
        event.get("target").and_then(|n| n.as_u64()),
        Some(buffer_entity.as_u64()),
        "event.target must match the buffer entity",
    );

    // SC-405 invariant: the BufferEdit was emitted by ActorIdentity::User.
    let provenance = event.get("provenance").expect("event.provenance present");
    let source = provenance.get("source").expect("provenance.source present");
    let source_type = source
        .get("type")
        .and_then(|s| s.as_str())
        .expect("provenance.source.type present");
    assert_eq!(
        source_type, "user",
        "SC-405: BufferEdit emitter MUST be ActorIdentity::User \
         (kebab-case discriminator on the wire); got source={source}",
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_open_bootstrap.rs / buffer_edit_single.rs)
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

async fn wait_for_buffer_version_one(
    observer: &mut Client,
    buffer_entity: weaver_core::types::entity_ref::EntityRef,
) {
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
        if fact.key.entity == buffer_entity
            && fact.key.attribute == "buffer/version"
            && let FactValue::U64(1) = fact.value
        {
            return;
        }
    }
    panic!("buffer/version=1 did not land within budget; edit may not have applied");
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
    let p = std::env::temp_dir().join(format!("weaver-edit-why-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-edit-why-e2e-{pid}-{tick}.sock"))
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
