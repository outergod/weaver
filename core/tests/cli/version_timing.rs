//! T075 — benchmark: the built `weaver --version` binary completes
//! within 50 ms (warm-cache, debug profile) per spec SC-006.
//!
//! The measurement brackets
//! `Command::new(CARGO_BIN_EXE_weaver).arg("--version").output()`
//! with `std::time::Instant`. One warm-up run primes filesystem and
//! linker caches; the assertion uses the median of five subsequent
//! runs to avoid noise from a single outlier (CI runners can
//! occasionally stall for tens of ms on process start).
//!
//! Reference: `specs/001-hello-fact/tasks.md` T075.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The contract budget from spec SC-006.
const BUDGET: Duration = Duration::from_millis(50);

/// How many samples after the warm-up. Median is reported to avoid
/// single-outlier failures.
const SAMPLES: usize = 5;

#[test]
fn version_output_completes_within_budget() {
    let binary = env!("CARGO_BIN_EXE_weaver");

    // Warm-up run — ignored.
    run_once(binary);

    let mut durations: Vec<Duration> = (0..SAMPLES).map(|_| run_once(binary)).collect();
    durations.sort();
    let median = durations[SAMPLES / 2];

    // Diagnostic — always print, helps diagnose regressions.
    eprintln!(
        "version_output timing (SAMPLES={SAMPLES}): min={:?} median={} max={:?}",
        durations.first().unwrap(),
        format_ms(median),
        durations.last().unwrap(),
    );

    assert!(
        median <= BUDGET,
        "median wall time {} exceeded SC-006 budget {}",
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
        .expect("spawn weaver --version");
    let elapsed = start.elapsed();
    assert!(out.status.success(), "weaver --version must exit 0");
    elapsed
}

fn format_ms(d: Duration) -> String {
    format!("{:.2}ms", (d.as_micros() as f64) / 1_000.0)
}
