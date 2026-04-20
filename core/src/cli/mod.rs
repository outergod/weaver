//! CLI entry points and subcommands.
//!
//! `clap` derive per L2 P6; `miette` errors with both human and JSON
//! rendering per L2 P5 / P6. Real CLI dispatch lands in T030+.

/// Process entry point — invoked from `core/src/main.rs`.
///
/// Stub: returns Ok immediately. Real subcommand dispatch lands in T030.
pub fn run() -> miette::Result<()> {
    Ok(())
}
