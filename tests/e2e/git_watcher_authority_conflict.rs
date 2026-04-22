//! T059 + review-fix F3 coverage: two watchers against the same repo
//! — the second must exit with code 3 within ~2s and must not
//! corrupt the first watcher's facts.
//!
//! The core's authority map rejects w2's asserts (F2 path); the
//! watcher's reader task (F3 fix) surfaces the `authority-conflict`
//! Error frame and exits. This test verifies both behaviours: w2's
//! process exit code AND w1's facts remaining attributed to w1's
//! instance identity.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const W2_EXIT_BUDGET: Duration = Duration::from_secs(5);

#[tokio::test]
async fn second_watcher_exits_code_3_and_leaves_first_watcher_facts_intact() {
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

    // Spawn w1 and let its bootstrap land.
    let _w1 = ChildGuard::new(spawn_watcher(&socket, &repo_dir));
    sleep(Duration::from_millis(500)).await;

    // Capture w1's instance id by reading a fact's provenance via a
    // scratch subscriber.
    let w1_instance = {
        let mut observer = Client::connect(&socket, "e2e-observer")
            .await
            .expect("observer connect");
        observer
            .subscribe(SubscribePattern::FamilyPrefix("repo/".into()))
            .await
            .expect("subscribe repo/");
        capture_instance(&mut observer).await
    };

    // Spawn w2 and wait for it to exit.
    let mut w2_child = spawn_watcher(&socket, &repo_dir);
    let w2_exit = wait_exit(&mut w2_child, W2_EXIT_BUDGET).await;

    let code = w2_exit.expect("w2 must exit within budget").code();
    assert_eq!(
        code,
        Some(3),
        "w2 must exit with code 3 (authority-conflict); got {code:?}",
    );

    // W1 must still own repo/* facts.
    let w1_still_owns = {
        let mut observer = Client::connect(&socket, "e2e-check")
            .await
            .expect("observer connect 2");
        observer
            .subscribe(SubscribePattern::FamilyPrefix("repo/".into()))
            .await
            .expect("subscribe repo/ 2");
        capture_instance(&mut observer).await
    };
    assert_eq!(
        w1_still_owns, w1_instance,
        "after w2 exited, w1's instance must still own repo/* facts; \
         instead saw {w1_still_owns:?} (was {w1_instance:?})",
    );
}

/// Subscribe to `repo/*` (caller already did this) and capture the
/// `instance_id` from the first `FactAssert` whose provenance is a
/// service identity. Used to verify authority ownership.
async fn capture_instance(client: &mut Client) -> Option<uuid::Uuid> {
    let deadline = Instant::now() + Duration::from_millis(1_500);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, client.recv()).await {
            Ok(Ok(BusMessage::FactAssert(f))) => {
                if let ActorIdentity::Service { instance_id, .. } = &f.provenance.source {
                    return Some(*instance_id);
                }
            }
            _ => return None,
        }
    }
    None
}

async fn wait_exit(
    child: &mut std::process::Child,
    budget: Duration,
) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < budget {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => sleep(Duration::from_millis(50)).await,
            Err(_) => return None,
        }
    }
    None
}

// ---- subprocess + helper boilerplate (mirrors tests/e2e/git_watcher.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-conf-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-conf-e2e-{pid}-{tick}.sock"))
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
