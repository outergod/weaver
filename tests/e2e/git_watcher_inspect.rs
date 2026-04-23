//! T066 — e2e scenario: start core + watcher on a git repo; run
//! `weaver inspect <repo-eref>:repo/dirty --output=json`; confirm
//! `asserting_kind = "service"`, `asserting_service = "git-watcher"`,
//! and `asserting_instance` is a valid UUID v4.
//!
//! The repo entity reference isn't known in advance (the watcher
//! derives it from the canonicalized repo path). We probe the fact
//! space via `weaver status --output=json` to learn the entity id,
//! then target that entity.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T066.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const FACT_WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn inspect_service_authored_repo_dirty_has_service_attribution() {
    let socket = unique_socket_path();
    let repo = prepare_repo();

    let core_bin = build_bin("weaver", "weaver_core");
    let watcher_bin = build_bin("weaver-git-watcher", "weaver-git-watcher");

    let mut core = spawn(&core_bin, &["run", "--socket", socket.to_str().unwrap()]);
    let _core_guard = ProcessGuard(core.id());
    wait_for_socket(&socket).await;

    let mut watcher = spawn(
        &watcher_bin,
        &[
            repo.path.to_str().unwrap(),
            "--socket",
            socket.to_str().unwrap(),
        ],
    );
    let _watcher_guard = ProcessGuard(watcher.id());

    // Wait for the watcher to publish its `repo/*` facts.
    let entity = wait_for_repo_entity(&socket).await;

    let inspect = run_weaver(
        &core_bin,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "inspect",
            &format!("{entity}:repo/dirty"),
            "--output=json",
        ],
    );
    assert!(
        inspect.status.success(),
        "inspect should succeed (status={:?}, stderr={})",
        inspect.status,
        String::from_utf8_lossy(&inspect.stderr),
    );
    let stdout = String::from_utf8(inspect.stdout).expect("utf-8");
    let v: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("non-JSON output: {e}\n{stdout}"));

    assert_eq!(
        v.get("asserting_kind").and_then(|k| k.as_str()),
        Some("service"),
        "expected asserting_kind=\"service\", got: {v}"
    );
    assert_eq!(
        v.get("asserting_service").and_then(|s| s.as_str()),
        Some("git-watcher"),
        "expected asserting_service=\"git-watcher\", got: {v}"
    );
    let instance = v
        .get("asserting_instance")
        .and_then(|i| i.as_str())
        .expect("asserting_instance must be present");
    let parsed = uuid::Uuid::parse_str(instance)
        .unwrap_or_else(|e| panic!("asserting_instance must parse as UUID: {e} / {instance:?}"));
    assert_eq!(
        parsed.get_version_num(),
        4,
        "asserting_instance must be a UUID v4, got version {:?}",
        parsed.get_version_num(),
    );
    assert!(
        v.get("asserting_behavior").is_none(),
        "service-authored fact must NOT carry asserting_behavior: {v}"
    );

    let _ = watcher.kill();
    let _ = watcher.wait();
    let _ = core.kill();
    let _ = core.wait();
    let _ = std::fs::remove_file(&socket);
}

struct PreparedRepo {
    path: PathBuf,
    // Kept alive so the tempdir survives the test.
    _td: tempfile::TempDir,
}

fn prepare_repo() -> PreparedRepo {
    let td = tempfile::tempdir().expect("tempdir");
    let path = td.path().to_path_buf();
    run_git(&path, &["init", "-b", "main", "-q"]);
    run_git(
        &path,
        &["config", "user.email", "inspect-e2e@example.invalid"],
    );
    run_git(&path, &["config", "user.name", "Inspect E2E"]);
    std::fs::write(path.join("a.txt"), "hello").unwrap();
    run_git(&path, &["add", "a.txt"]);
    run_git(&path, &["commit", "-q", "-m", "initial"]);
    PreparedRepo { path, _td: td }
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

/// Poll `weaver status --output=json` until a `repo/dirty` fact
/// appears; return its entity id.
async fn wait_for_repo_entity(socket: &Path) -> u64 {
    let core_bin = weaver_bin("weaver");
    let deadline = Instant::now() + FACT_WAIT;
    loop {
        if Instant::now() >= deadline {
            panic!("repo/dirty fact did not appear within {FACT_WAIT:?}");
        }
        let out = Command::new(&core_bin)
            .args(["--socket", socket.to_str().unwrap(), "status", "-o", "json"])
            .output()
            .expect("status runs");
        if out.status.success() {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(facts) = v.get("facts").and_then(|f| f.as_array()) {
                    for f in facts {
                        let attribute = f
                            .get("key")
                            .and_then(|k| k.get("attribute"))
                            .and_then(|a| a.as_str());
                        let entity = f
                            .get("key")
                            .and_then(|k| k.get("entity"))
                            .and_then(|e| e.as_u64());
                        if attribute == Some("repo/dirty") {
                            if let Some(e) = entity {
                                return e;
                            }
                        }
                    }
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn run_weaver(bin: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver")
}

fn spawn(bin: &Path, args: &[&str]) -> std::process::Child {
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn subprocess")
}

fn build_bin(bin_name: &str, package: &str) -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", package, "--bin", bin_name])
        .status()
        .expect("cargo build");
    assert!(status.success(), "cargo build {bin_name} failed");
    weaver_bin(bin_name)
}

fn weaver_bin(name: &str) -> PathBuf {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("workspace root")
                .join("target")
        });
    target_dir.join("debug").join(name)
}

async fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while !socket.exists() {
        if start.elapsed() > SOCKET_WAIT_TIMEOUT {
            panic!("socket did not appear within {SOCKET_WAIT_TIMEOUT:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
    sleep(Duration::from_millis(50)).await;
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("weaver-e2e-inspect-{pid}-{tick}.sock"))
}

/// Best-effort cleanup. SIGTERM first so the watcher gets its
/// retract path; SIGKILL after a short grace.
struct ProcessGuard(u32);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Safety: libc::kill with a scalar pid and signal.
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(50));
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
        }
    }
}
