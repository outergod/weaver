//! Tracing subscriber initialization (L2 P13 — observability for operators).
//!
//! Structured spans + line events written to stderr. Verbosity is
//! controllable via `RUST_LOG` (preferred) or the `-v` CLI flag (one
//! step per repetition).

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init(verbose_count: u8) {
    // RUST_LOG wins if set; CLI verbosity is the fallback.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = match verbose_count {
            0 => "info",
            1 => "debug",
            _ => "trace",
        };
        EnvFilter::new(format!("weaver={level},weaver_core={level},weaver_tui={level}"))
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
        .try_init();
}
