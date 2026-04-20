//! Weaver TUI library — bus client, renderer, in-TUI commands.
//!
//! See `specs/001-hello-fact/contracts/cli-surfaces.md` for the rendered
//! shape. Slice 001 Phase 2 ships the connection + handshake + subscribe
//! flow; interactive crossterm raw-mode and fact rendering land in
//! Phase 3 (T036/T047) and disconnect handling in Phase 3 (T071/T072).

pub mod args;
pub mod client;
pub mod commands;
pub mod render;

use clap::Parser;
use miette::IntoDiagnostic;

/// TUI entry point — invoked from `tui/src/main.rs`.
pub fn run() -> miette::Result<()> {
    let cli = args::Cli::parse();

    if cli.version {
        print_version(cli.no_color);
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;

    runtime.block_on(async move {
        let socket = cli.socket.unwrap_or_else(default_socket_path);
        render::run(socket).await
    })
}

/// Minimal `--version` output for the TUI binary (mirrors `weaver
/// --version` shape for consistency).
fn print_version(_no_color: bool) {
    println!("weaver-tui {}", env!("CARGO_PKG_VERSION"));
    println!("  bus protocol: v{}", weaver_core::types::message::BUS_PROTOCOL_VERSION_STR);
}

fn default_socket_path() -> std::path::PathBuf {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        std::path::Path::new(&xdg).join("weaver.sock")
    } else {
        std::path::PathBuf::from("/tmp/weaver.sock")
    }
}
