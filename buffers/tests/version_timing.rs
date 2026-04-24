//! T069 — benchmark: the built `weaver-buffers --version` binary
//! completes within 50 ms (warm-cache, debug profile), matching the
//! slice-001 `weaver --version` budget pinned by T075 of slice 001.
//!
//! Method mirrors `core/tests/cli/version_timing.rs`: one warm-up run
//! primes filesystem and linker caches; the assertion uses the median
//! of five subsequent runs to avoid single-outlier noise (CI runners
//! occasionally stall tens of ms on process start). Observed
//! min/median/max are printed to stderr so regressions diagnose from
//! the test output alone, without re-running the bench by hand.
//!
//! File lives under `buffers/tests/` rather than the `core/tests/cli/`
//! path named in `specs/003-buffer-service/tasks.md` T069:
//! `env!("CARGO_BIN_EXE_weaver-buffers")` only resolves in integration
//! tests of the crate that defines the `weaver-buffers` binary — which
//! is `buffers`, not `core`. The benchmark's intent is preserved;
//! cargo's binary-env-var semantics dictate the location.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Shared budget with `weaver --version` — the CLI-surface contract
/// in `contracts/cli-surfaces.md` holds `--version` invocations to
/// the same interactive-class budget across every Weaver binary.
const BUDGET: Duration = Duration::from_millis(50);

/// Samples after the warm-up run; median is reported and asserted.
const SAMPLES: usize = 5;

#[test]
fn weaver_buffers_version_output_completes_within_budget() {
    let binary = env!("CARGO_BIN_EXE_weaver-buffers");

    // Warm-up — ignored. Primes the kernel's inode/page cache and the
    // dynamic linker's symbol tables so the measured samples reflect
    // steady-state cost, not first-invocation overhead.
    run_once(binary);

    let mut durations: Vec<Duration> = (0..SAMPLES).map(|_| run_once(binary)).collect();
    durations.sort();
    let median = durations[SAMPLES / 2];

    eprintln!(
        "weaver-buffers --version timing (SAMPLES={SAMPLES}): min={:?} median={} max={:?}",
        durations.first().unwrap(),
        format_ms(median),
        durations.last().unwrap(),
    );

    assert!(
        median <= BUDGET,
        "median wall time {} exceeded T069 budget {}",
        format_ms(median),
        format_ms(BUDGET),
    );
}

fn run_once(binary: &str) -> Duration {
    let start = Instant::now();
    let out = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("spawn weaver-buffers --version");
    let elapsed = start.elapsed();
    assert!(out.status.success(), "weaver-buffers --version must exit 0");
    elapsed
}

fn format_ms(d: Duration) -> String {
    format!("{:.2}ms", (d.as_micros() as f64) / 1_000.0)
}
