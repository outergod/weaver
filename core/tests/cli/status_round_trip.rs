//! T056 — `weaver status --output=json` produces JSON parseable as
//! [`StatusResponse`] (in `weaver_core::cli::output`) and round-trips
//! cleanly back to JSON.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T056.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

use weaver_core::cli::output::StatusResponse;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn status_json_round_trips_via_serde() {
    let socket = unique_socket_path("roundtrip");
    let mut core = spawn_core(&socket);
    let _core_guard = ProcessGuard(core.id());
    wait_for_socket(&socket).await;

    let output = run_weaver(&["--socket", socket.to_str().unwrap(), "status", "-o", "json"]);
    assert!(
        output.status.success(),
        "weaver status -o json should succeed when core is reachable (status={:?}, stderr={})",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");

    let parsed: StatusResponse = serde_json::from_str(&stdout).expect("parseable JSON");
    assert_eq!(parsed.lifecycle, "ready");
    assert!(
        parsed.uptime_ns.is_some(),
        "ready shape must include uptime_ns",
    );
    assert!(parsed.error.is_none());

    // Round-trip back through serde and compare.
    let reserialized = serde_json::to_string(&parsed).unwrap();
    let back: StatusResponse = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(parsed, back, "round-trip must preserve all fields");

    // Clean up core process.
    let _ = core.kill();
    let _ = core.wait();
    let _ = std::fs::remove_file(&socket);
}

fn run_weaver(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_weaver"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver")
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

fn unique_socket_path(tag: &str) -> PathBuf {
    let pid = std::process::id();
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("weaver-status-{tag}-{pid}-{tick}.sock"))
}

/// Best-effort cleanup for the spawned core process.
struct ProcessGuard(u32);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Best-effort — if kill fails because the process already
        // exited, we simply move on.
        let _ = Command::new("kill")
            .arg(format!("{}", self.0))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
