//! `clap` derive structures for the `weaver` binary.
//!
//! See `specs/001-hello-fact/contracts/cli-surfaces.md` for the contract.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "weaver",
    about = "Weaver core — bus, fact space, behavior dispatcher",
    long_about = None,
    disable_version_flag = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Print build provenance (per L2 P11). Use with `-o json` for the
    /// machine-readable form.
    #[arg(short = 'V', long)]
    pub version: bool,

    /// Increase log verbosity (repeat: `-v`, `-vv`, `-vvv`).
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Output format for commands that produce structured output.
    #[arg(
        short = 'o',
        long,
        value_enum,
        default_value_t = OutputFormat::Human,
        global = true,
    )]
    pub output: OutputFormat,

    /// Override the default bus socket path.
    /// Defaults to `$XDG_RUNTIME_DIR/weaver.sock` (or `/tmp/weaver.sock`
    /// when `$XDG_RUNTIME_DIR` is unset).
    #[arg(long, global = true, env = "WEAVER_SOCKET")]
    pub socket: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Start the core process; block until SIGINT/SIGTERM.
    Run,

    /// One-shot snapshot of core lifecycle and currently-asserted facts.
    Status,

    /// Inspect a fact's provenance.
    Inspect {
        /// Fact key in `<entity-id>:<attribute>` format
        /// (e.g., `1:buffer/dirty`).
        fact_key: String,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Human-formatted output (default).
    Human,
    /// Machine-readable JSON.
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_clap_definition_is_valid() {
        // Catches `clap` derive misconfiguration at test time rather than
        // at first user invocation.
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_run_subcommand() {
        let cli = Cli::try_parse_from(["weaver", "run"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Run)));
    }

    #[test]
    fn parses_short_output_alias() {
        let cli = Cli::try_parse_from(["weaver", "-o", "json", "status"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
        assert!(matches!(cli.command, Some(Command::Status)));
    }

    #[test]
    fn parses_long_output_with_equals() {
        let cli = Cli::try_parse_from(["weaver", "--output=json", "status"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn version_flag_recognized() {
        let cli = Cli::try_parse_from(["weaver", "--version"]).unwrap();
        assert!(cli.version);
    }
}
