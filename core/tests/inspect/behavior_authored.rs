//! T065 — scenario test: `weaver inspect 1:buffer/dirty --output=json`
//! returns a JSON object whose `asserting_kind` is `"behavior"` and
//! whose `asserting_behavior` names `core/dirty-tracking`. No
//! `asserting_service` / `asserting_instance` fields are present.
//!
//! Drives the full CLI stack (core binary + client + inspect
//! command) rather than calling `inspect_fact` directly so the JSON
//! envelope (`FoundJson` in `cli/inspect.rs`) is exercised
//! end-to-end.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T065.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn inspect_behavior_authored_fact_shape_matches_contract() {
    let socket = unique_socket_path("behavior-authored");
    let mut core = spawn_core(&socket);
    let _guard = ProcessGuard(core.id());
    wait_for_socket(&socket).await;

    // Trigger the dirty-tracking behavior via `weaver simulate-edit 1`
    // so `buffer/dirty` is asserted by `core/dirty-tracking`.
    let edit = run_weaver(&["--socket", socket.to_str().unwrap(), "simulate-edit", "1"]);
    assert!(
        edit.status.success(),
        "simulate-edit should succeed (stderr={})",
        String::from_utf8_lossy(&edit.stderr),
    );
    // Give the dispatcher a brief window to apply the behavior and
    // persist the FactAsserted trace entry before we inspect.
    sleep(Duration::from_millis(50)).await;

    let inspect = run_weaver(&[
        "--socket",
        socket.to_str().unwrap(),
        "inspect",
        "1:buffer/dirty",
        "--output=json",
    ]);
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
        Some("behavior"),
        "expected asserting_kind=\"behavior\", got: {v}"
    );
    assert_eq!(
        v.get("asserting_behavior").and_then(|b| b.as_str()),
        Some("core/dirty-tracking"),
        "expected asserting_behavior=core/dirty-tracking, got: {v}"
    );
    assert!(
        v.get("asserting_service").is_none(),
        "behavior-authored fact must NOT carry asserting_service: {v}"
    );
    assert!(
        v.get("asserting_instance").is_none(),
        "behavior-authored fact must NOT carry asserting_instance: {v}"
    );

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
    std::env::temp_dir().join(format!("weaver-inspect-{tag}-{pid}-{tick}.sock"))
}

/// Best-effort cleanup for the spawned core process.
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
