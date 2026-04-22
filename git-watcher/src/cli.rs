//! CLI surface for `weaver-git-watcher`. See
//! `specs/002-git-watcher-actor/contracts/cli-surfaces.md`.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use miette::{IntoDiagnostic, Report};
use thiserror::Error;
use tokio::runtime::Builder;

use crate::observer::RepoObserver;
use crate::publisher::{self, PublisherError};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Exit codes per `contracts/cli-surfaces.md`:
///
/// * 0 — clean exit (SIGTERM / SIGINT received; facts retracted).
/// * 1 — fatal startup error (repo path invalid, not a git repo,
///   permission denied).
/// * 2 — bus unavailable.
/// * 3 — authority conflict (another watcher already claims the repo).
/// * 10 — unrecoverable internal error.
pub mod exit_code {
    pub const STARTUP_FAILURE: i32 = 1;
    pub const BUS_UNAVAILABLE: i32 = 2;
    pub const AUTHORITY_CONFLICT: i32 = 3;
    pub const INTERNAL: i32 = 10;
}

#[derive(Parser, Debug)]
#[command(
    name = "weaver-git-watcher",
    about = "Weaver git-watcher: first non-editor service actor on the bus",
    long_about = "Observes one local git repository and publishes authoritative \
                  repo/* facts (dirty, head-commit, working-copy state) under a \
                  structured ActorIdentity::Service on the Weaver bus. See \
                  specs/002-git-watcher-actor/ for the specification."
)]
pub struct Cli {
    /// Path to the git repository's working-tree root. Not required
    /// when `--version` is supplied.
    #[arg(required_unless_present = "version")]
    pub repository: Option<PathBuf>,

    /// Observation cadence. Humantime-parsed (e.g. `100ms`, `1s`).
    #[arg(long, default_value = "250ms")]
    pub poll_interval: String,

    /// Override the bus socket path. Mirrors core's lookup order:
    /// `--socket` wins, then the `WEAVER_SOCKET` env var, then the
    /// XDG/`/tmp` default. Keeping env parity with `weaver run`
    /// means a watcher launched without `--socket` will find the
    /// same socket core writes to (F20 review fix).
    #[arg(long, env = "WEAVER_SOCKET")]
    pub socket: Option<PathBuf>,

    /// Output format for startup logs and `--version`.
    #[arg(long, short = 'o', default_value = "human")]
    pub output: String,

    /// Increase log verbosity (repeatable: -v, -vv, -vvv).
    #[arg(long, short = 'v', action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Print contracted version output (human or JSON per `--output`)
    /// and exit. Routes through this crate's own renderer rather than
    /// clap's built-in single-line action so the output matches
    /// `contracts/cli-surfaces.md` (commit/dirty/built/profile/rustc/
    /// bus_protocol/service_id fields).
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub version: bool,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("--poll-interval: {source}")]
    BadPollInterval {
        #[source]
        source: humantime::DurationError,
    },

    #[error(
        "--poll-interval must be > 0: `humantime` accepts `0ms`, but \
         `tokio::time::interval` panics on a zero period. Use e.g. `1ms`."
    )]
    ZeroPollInterval,
}

/// Parse the CLI, initialize tracing, and run the publisher until the
/// process is signalled.
pub fn run() -> Result<(), Report> {
    let cli = Cli::parse();

    // F5 review fix: render the contracted version shape before any
    // runtime setup. clap's built-in version action would have exited
    // here producing a single-line string; routing through our own
    // renderer honours `--output=json|human` per the CLI contract.
    if cli.version {
        crate::version::print_version(&cli.output);
        return Ok(());
    }

    init_tracing(cli.verbose, &cli.output);

    let poll_interval = parse_duration(&cli.poll_interval).into_diagnostic()?;
    let repo_path = cli
        .repository
        .clone()
        .expect("clap requires repository unless --version is set");
    let socket_override = cli.socket.clone();

    // T038: fail fast if the target is not a git repository.
    let observer = match RepoObserver::open(&repo_path) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(exit_code::STARTUP_FAILURE);
        }
    };

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;

    let outcome = runtime
        .block_on(async move { publisher::run(observer, socket_override, poll_interval).await });

    match outcome {
        Ok(()) => Ok(()),
        Err(PublisherError::BusUnavailable { .. }) => {
            eprintln!("error: {}", outcome.unwrap_err());
            std::process::exit(exit_code::BUS_UNAVAILABLE);
        }
        Err(PublisherError::AuthorityConflict { .. }) => {
            eprintln!("error: {}", outcome.unwrap_err());
            std::process::exit(exit_code::AUTHORITY_CONFLICT);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(exit_code::INTERNAL);
        }
    }
}

/// Parse a humantime-style duration string (e.g. `250ms`, `1s`).
///
/// F16 review fix: `humantime` parses `0ms` as `Duration::ZERO`,
/// which would otherwise reach `tokio::time::interval` and panic
/// at runtime. Reject zero up front so the CLI fails via the
/// documented startup-error path (exit 1) instead.
fn parse_duration(input: &str) -> Result<Duration, CliError> {
    let parsed =
        humantime::parse_duration(input).map_err(|source| CliError::BadPollInterval { source })?;
    if parsed.is_zero() {
        return Err(CliError::ZeroPollInterval);
    }
    Ok(parsed)
}

fn init_tracing(verbose: u8, output: &str) {
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("weaver_git_watcher={default_level}")));
    // F17 review fix: `--output` advertises control over both
    // `--version` rendering AND runtime logs, but init_tracing
    // previously hardcoded the human formatter, so scripted
    // consumers asking for JSON still received human lines.
    // Route the value here to honour the contract.
    let builder = fmt().with_env_filter(filter).with_writer(std::io::stderr);
    let _ = match output {
        "json" => builder.json().try_init(),
        // Unknown formats fall back to human, matching `print_version`'s
        // lenient policy so `--output=garbage` still produces useful
        // output rather than a startup error.
        _ => builder.try_init(),
    };
}

// Keep the constant referenced so future plumbing adopts the default.
const _: Duration = DEFAULT_POLL_INTERVAL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_valid_humantime() {
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn parse_duration_rejects_zero() {
        assert!(matches!(
            parse_duration("0ms").unwrap_err(),
            CliError::ZeroPollInterval
        ));
        assert!(matches!(
            parse_duration("0s").unwrap_err(),
            CliError::ZeroPollInterval
        ));
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(matches!(
            parse_duration("not-a-duration").unwrap_err(),
            CliError::BadPollInterval { .. }
        ));
    }
}
