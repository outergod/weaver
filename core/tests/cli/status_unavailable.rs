//! T057 — with no core running, `weaver status --output=json` prints
//! the documented `{"lifecycle": "unavailable", "error": "..."}` shape
//! and exits with code 2.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T057 +
//! `specs/001-hello-fact/contracts/cli-surfaces.md`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use weaver_core::cli::output::StatusResponse;

#[test]
fn status_unavailable_emits_documented_shape_and_exit_code_2() {
    // Guaranteed-nonexistent socket path.
    let socket = unique_socket_path();

    let output = Command::new(env!("CARGO_BIN_EXE_weaver"))
        .args(["--socket", socket.to_str().unwrap(), "status", "-o", "json"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver");

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let parsed: StatusResponse =
        serde_json::from_str(&stdout).expect("unavailable shape must still be valid JSON");

    assert_eq!(parsed.lifecycle, "unavailable");
    assert!(parsed.uptime_ns.is_none());
    assert!(parsed.facts.is_empty());
    let err_msg = parsed
        .error
        .as_deref()
        .expect("unavailable shape must include an `error` field");
    assert!(
        err_msg.contains(socket.to_str().unwrap()),
        "error message must name the socket path; got `{err_msg}`",
    );

    let code = output
        .status
        .code()
        .expect("process exited via a code (not a signal)");
    assert_eq!(
        code, 2,
        "exit code 2 per cli-surfaces.md (core-unavailable)",
    );
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("weaver-status-unavailable-{pid}-{tick}.sock"))
}
