//! T049 — US2 e2e: `weaver inspect <buffer-entity>:buffer/dirty
//! --output=json` must attribute the fact to the `weaver-buffers`
//! service (service-authored provenance; no behavior carrier).
//!
//! Three-process scenario:
//!   1. Spawn `weaver run`; wait for socket.
//!   2. Spawn `weaver-buffers <fixture>` pointed at the same socket.
//!   3. Poll `weaver status --output=json` until a `buffer/dirty`
//!      fact is visible, confirming the bootstrap has landed.
//!   4. Invoke `weaver inspect <entity>:buffer/dirty --output=json`.
//!
//! The buffer entity is derived in-test via `weaver_buffers::model::
//! buffer_entity_ref(canonical)`; if the service's derivation ever
//! drifts from the library function (e.g., alternate canonicalization),
//! this test surfaces the divergence as a fact-not-found at inspect.
//!
//! SC-305 coverage. Mirrors `tests/e2e/git_watcher_inspect.rs` shape.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

use weaver_buffers::model::buffer_entity_ref;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const FACT_WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn inspect_service_authored_buffer_dirty_has_weaver_buffers_attribution() {
    let socket = unique_socket_path();

    let core_bin = build_bin("weaver", "weaver_core");
    let buffers_bin = build_bin("weaver-buffers", "weaver-buffers");

    let mut core = spawn(&core_bin, &["run", "--socket", socket.to_str().unwrap()]);
    let _core_guard = ProcessGuard(core.id());
    wait_for_socket(&socket).await;

    let fixture_dir = tempfile::tempdir().expect("tempdir");
    let fixture_path = fixture_dir.path().join("fixture.txt");
    std::fs::write(&fixture_path, b"hello buffer\n").expect("write fixture");
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");
    let entity = buffer_entity_ref(&canonical_fixture).as_u64();

    let mut buffers = spawn(
        &buffers_bin,
        &[
            canonical_fixture.to_str().unwrap(),
            "--socket",
            socket.to_str().unwrap(),
            "--poll-interval=100ms",
        ],
    );
    let _buffers_guard = ProcessGuard(buffers.id());

    wait_for_buffer_dirty(&core_bin, &socket, entity).await;

    let inspect = run_weaver(
        &core_bin,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "inspect",
            &format!("{entity}:buffer/dirty"),
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
        Some("weaver-buffers"),
        "expected asserting_service=\"weaver-buffers\", got: {v}"
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

    let _ = buffers.kill();
    let _ = buffers.wait();
    let _ = core.kill();
    let _ = core.wait();
    let _ = std::fs::remove_file(&socket);
}

/// Poll `weaver status --output=json` until `buffer/dirty` appears
/// for the expected entity. Returns on first match; panics on
/// deadline.
async fn wait_for_buffer_dirty(core_bin: &Path, socket: &Path, expected_entity: u64) {
    let deadline = Instant::now() + FACT_WAIT;
    loop {
        if Instant::now() >= deadline {
            panic!("buffer/dirty fact did not appear within {FACT_WAIT:?}");
        }
        let out = Command::new(core_bin)
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
                        if attribute == Some("buffer/dirty") && entity == Some(expected_entity) {
                            return;
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
    std::env::temp_dir().join(format!("weaver-buf-inspect-{pid}-{tick}.sock"))
}

/// Best-effort cleanup. SIGTERM first so the service gets its
/// retract path; SIGKILL after a short grace.
struct ProcessGuard(u32);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(50));
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
        }
    }
}
