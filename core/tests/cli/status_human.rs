//! T058 — `weaver status --output=human` produces human output
//! containing the lifecycle state and the fact count.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T058.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn status_human_shows_lifecycle_and_fact_count() {
    let socket = unique_socket_path();
    let mut core = spawn_core(&socket);
    let _guard = ProcessGuard(core.id());
    wait_for_socket(&socket).await;

    let output = Command::new(env!("CARGO_BIN_EXE_weaver"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "status",
            "-o",
            "human",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver status");
    assert!(
        output.status.success(),
        "weaver status -o human must succeed"
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains("lifecycle: ready"),
        "human output must surface the lifecycle state, got:\n{stdout}",
    );
    assert!(
        stdout.contains("facts (0)") || stdout.contains("facts ("),
        "human output must surface a fact count, got:\n{stdout}",
    );

    let _ = core.kill();
    let _ = core.wait();
    let _ = std::fs::remove_file(&socket);
}

fn spawn_core(socket: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_weaver"))
        .arg("run")
        .arg("--socket")
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver run")
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
    std::env::temp_dir().join(format!("weaver-status-human-{pid}-{tick}.sock"))
}

struct ProcessGuard(u32);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg(format!("{}", self.0))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
