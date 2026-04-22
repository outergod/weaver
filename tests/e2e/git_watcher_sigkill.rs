//! Review-fix F1 coverage: SIGKILL-ing the watcher retracts every
//! `repo/*` fact it asserted.
//!
//! Without F1 a crashed or SIGKILLed publisher leaves its
//! authoritative facts behind forever — a replacement watcher
//! starting in a different `repo/state/*` variant would run alongside
//! the dead variant, breaking the mutex invariant across connection
//! boundaries. This test spawns the watcher, observes its bootstrap,
//! SIGKILLs it, and verifies the subscriber sees retracts for every
//! fact the watcher asserted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const RETRACT_BUDGET: Duration = Duration::from_secs(3);

#[tokio::test]
async fn sigkill_retracts_every_fact_the_watcher_asserted() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;
    build_git_watcher_binary();

    // Fresh repo with one commit on `main`.
    let repo_dir = tempdir();
    git(&repo_dir, &["init", "-b", "main", "-q"]);
    git(&repo_dir, &["config", "user.email", "e2e@test.invalid"]);
    git(&repo_dir, &["config", "user.name", "e2e"]);
    std::fs::write(repo_dir.join("a.txt"), "hello").unwrap();
    git(&repo_dir, &["add", "a.txt"]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);

    // Subscribe before starting the watcher.
    let mut observer = Client::connect(&socket, "e2e-sigkill")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // Spawn the watcher and let its bootstrap reach the observer.
    // The `ChildGuard` handles both SIGKILL reaping (when the test
    // kills the process below, the guard's drop path still finds the
    // already-dead child and `wait()`s it, avoiding zombies).
    let watcher_child = spawn_watcher(&socket, &repo_dir);
    let watcher_pid = watcher_child.id();
    let _watcher = ChildGuard::new(watcher_child);
    let mut asserted: HashSet<String> = HashSet::new();
    let deadline = Instant::now() + Duration::from_millis(2_000);
    while Instant::now() < deadline {
        match timeout(Duration::from_millis(200), observer.recv()).await {
            Ok(Ok(BusMessage::FactAssert(f))) => {
                asserted.insert(f.key.attribute.clone());
            }
            _ => continue,
        }
        // Once all six bootstrap attrs land, stop early.
        if ["repo/path", "repo/dirty", "repo/head-commit"]
            .iter()
            .all(|a| asserted.contains(*a))
        {
            break;
        }
    }
    assert!(
        asserted.contains("repo/dirty"),
        "bootstrap did not assert repo/dirty; got {asserted:?}",
    );

    // SIGKILL the watcher — no clean shutdown.
    let _ = nix_signal(watcher_pid, libc::SIGKILL);
    // Collect retractions for up to RETRACT_BUDGET.
    let mut retracted: HashSet<String> = HashSet::new();
    let deadline = Instant::now() + RETRACT_BUDGET;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match timeout(remaining, observer.recv()).await {
            Ok(Ok(BusMessage::FactRetract { key, .. })) => {
                retracted.insert(key.attribute.clone());
            }
            Ok(Ok(_)) => continue,
            _ => break,
        }
        if asserted.is_subset(&retracted) {
            break;
        }
    }
    // Every fact the watcher asserted during bootstrap must now be
    // retracted. (Some Lifecycle messages may interleave; they don't
    // participate in the retract set.)
    for attr in &asserted {
        assert!(
            retracted.contains(attr),
            "expected retract of {attr} after SIGKILL; retracted={retracted:?}",
        );
    }

    // ChildGuard's drop reaps the already-dead watcher process.
    drop(_watcher);
    // `watcher_pid` is kept around to silence "unused" warnings if
    // future assertions want to reference it.
    let _ = watcher_pid;
}

// ---- subprocess + helper boilerplate (mirrors tests/e2e/git_watcher.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-sigkill-e2e-{pid}-{tick}"));
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

fn spawn_watcher(socket: &Path, repo: &Path) -> std::process::Child {
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
    std::env::temp_dir().join(format!("weaver-sigkill-e2e-{pid}-{tick}.sock"))
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
