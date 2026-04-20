//! Weaver TUI library — bus client, crossterm-based renderer, in-TUI commands.
//!
//! See `specs/001-hello-fact/contracts/cli-surfaces.md` for the rendered shape.
//! Module bodies are populated in T035 / T036 / T037 / T046+.

pub mod client;
pub mod render;
pub mod commands;

/// TUI entry point — invoked from `tui/src/main.rs`.
///
/// Stub: returns Ok immediately. Real loop lands in later tasks.
pub fn run() -> miette::Result<()> {
    Ok(())
}
