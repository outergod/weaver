//! T050 — US2 e2e: slice 003 removed `weaver simulate-edit` and
//! `weaver simulate-clean` (see `specs/003-buffer-service/
//! contracts/cli-surfaces.md §REMOVED subcommands`). This test
//! pins the clap-level failure shape so a regression that silently
//! re-adds a subcommand would surface here.
//!
//! No core is spawned — clap rejects the subcommand before the
//! runtime is touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[test]
fn simulate_edit_is_an_unrecognized_subcommand() {
    let weaver = build_weaver_binary();
    let out = Command::new(&weaver)
        .args(["simulate-edit", "1"])
        .stdin(Stdio::null())
        .output()
        .expect("run weaver");
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected clap exit 2, got {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).expect("utf-8");
    assert!(
        stderr.contains("unrecognized subcommand"),
        "stderr must mention `unrecognized subcommand`; got:\n{stderr}"
    );
    assert!(
        stderr.contains("simulate-edit"),
        "stderr must name the rejected subcommand; got:\n{stderr}"
    );
}

#[test]
fn simulate_clean_is_an_unrecognized_subcommand() {
    let weaver = build_weaver_binary();
    let out = Command::new(&weaver)
        .args(["simulate-clean", "1"])
        .stdin(Stdio::null())
        .output()
        .expect("run weaver");
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected clap exit 2, got {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).expect("utf-8");
    assert!(
        stderr.contains("unrecognized subcommand"),
        "stderr must mention `unrecognized subcommand`; got:\n{stderr}"
    );
    assert!(
        stderr.contains("simulate-clean"),
        "stderr must name the rejected subcommand; got:\n{stderr}"
    );
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success(), "cargo build weaver failed");
    weaver_bin_path()
}

fn weaver_bin_path() -> PathBuf {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("workspace root")
                .join("target")
        });
    target_dir.join("debug").join("weaver")
}
