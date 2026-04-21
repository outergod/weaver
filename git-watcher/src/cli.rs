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
    version,
    about = "Weaver git-watcher: first non-editor service actor on the bus",
    long_about = "Observes one local git repository and publishes authoritative \
                  repo/* facts (dirty, head-commit, working-copy state) under a \
                  structured ActorIdentity::Service on the Weaver bus. See \
                  specs/002-git-watcher-actor/ for the specification."
)]
pub struct Cli {
    /// Path to the git repository's working-tree root.
    pub repository: PathBuf,

    /// Observation cadence. Humantime-parsed (e.g. `100ms`, `1s`).
    #[arg(long, default_value = "250ms")]
    pub poll_interval: String,

    /// Override the bus socket path.
    #[arg(long)]
    pub socket: Option<PathBuf>,

    /// Output format for startup logs and --version.
    #[arg(long, short = 'o', default_value = "human")]
    pub output: String,

    /// Increase log verbosity (repeatable: -v, -vv, -vvv).
    #[arg(long, short = 'v', action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("--poll-interval: {source}")]
    BadPollInterval {
        #[source]
        source: humantime::DurationError,
    },
}

/// Parse the CLI, initialize tracing, and run the publisher until the
/// process is signalled.
pub fn run() -> Result<(), Report> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    let poll_interval = parse_duration(&cli.poll_interval).into_diagnostic()?;
    let repo_path = cli.repository.clone();
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
fn parse_duration(input: &str) -> Result<Duration, CliError> {
    humantime::parse_duration(input).map_err(|source| CliError::BadPollInterval { source })
}

fn init_tracing(verbose: u8) {
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("weaver_git_watcher={default_level}")));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

// Keep the constant referenced so future plumbing adopts the default.
const _: Duration = DEFAULT_POLL_INTERVAL;
