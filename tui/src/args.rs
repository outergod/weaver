//! `clap` derive structures for the `weaver-tui` binary.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "weaver-tui",
    about = "Weaver TUI — bus client + terminal renderer",
    long_about = None,
    disable_version_flag = true,
)]
pub struct Cli {
    /// Print build provenance.
    #[arg(short = 'V', long)]
    pub version: bool,

    /// Override the default bus socket path.
    #[arg(long, env = "WEAVER_SOCKET")]
    pub socket: Option<PathBuf>,

    /// Disable ANSI colors in output.
    #[arg(long)]
    pub no_color: bool,
}
