//! CLI surface for `weaver-buffers`. See
//! `specs/003-buffer-service/contracts/cli-surfaces.md`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use miette::{Diagnostic, IntoDiagnostic, Report};
use thiserror::Error;
use tokio::runtime::Builder;
use tracing::debug;

use crate::model::{ObserverError, StartupKind};
use crate::publisher::{self, PublisherError};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Exit codes per `contracts/cli-surfaces.md §Exit codes`.
pub mod exit_code {
    pub const STARTUP_FAILURE: i32 = 1;
    pub const BUS_UNAVAILABLE: i32 = 2;
    pub const AUTHORITY_CONFLICT: i32 = 3;
    pub const INTERNAL: i32 = 10;
}

#[derive(Parser, Debug)]
#[command(
    name = "weaver-buffers",
    about = "Weaver buffer service: first content-backed service actor on the bus",
    long_about = "Opens one or more regular files named on the CLI and publishes \
                  authoritative buffer/* facts (path, byte-size, dirty, observable) \
                  under a structured ActorIdentity::Service on the Weaver bus. See \
                  specs/003-buffer-service/ for the specification."
)]
pub struct Cli {
    /// One or more paths to regular files. Required unless `--version`
    /// is supplied. Duplicate canonical paths collapse to one buffer
    /// entity (FR-006a) — see [`canonicalise_and_dedup`].
    #[arg(required_unless_present = "version", num_args = 1..)]
    pub paths: Vec<PathBuf>,

    /// Observation cadence. Humantime-parsed (e.g. `100ms`, `1s`).
    #[arg(long, default_value = "250ms")]
    pub poll_interval: String,

    /// Override the bus socket path. Mirrors core's lookup order:
    /// `--socket` wins, then the `WEAVER_SOCKET` env var, then the
    /// XDG/`/tmp` default.
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
    /// clap's built-in one-liner so the output matches the CLI
    /// contract's commit/dirty/built/profile/rustc/bus_protocol/
    /// service_id fields.
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

/// Structured startup diagnostics rendered via `miette`. Maps each
/// categorised [`ObserverError::StartupFailure`] kind plus the
/// publisher's authority-conflict case to the CLI contract's
/// `WEAVER-BUF-00{1,2,3,4}` codes.
#[derive(Debug, Error, Diagnostic)]
enum StartupDiagnostic {
    #[error("buffer not openable at {path}: {reason}")]
    #[diagnostic(
        code("WEAVER-BUF-001"),
        help(
            "no fact (entity:<derived>, attribute:buffer/path) can be asserted.\nPoint weaver-buffers at a regular file whose content you want to observe."
        )
    )]
    NotOpenable { path: String, reason: String },

    #[error("buffer not openable at {path}: path is not a regular file")]
    #[diagnostic(
        code("WEAVER-BUF-002"),
        help(
            "weaver-buffers only opens regular files in slice 003.\nDirectory-level observation belongs to a future slice."
        )
    )]
    NotRegularFile { path: String },

    #[error("buffer not openable at {path}: file size exceeds available memory")]
    #[diagnostic(
        code("WEAVER-BUF-003"),
        help(
            "slice 003 reads each buffer's content fully into memory at open time.\nStreaming-open is not yet supported."
        )
    )]
    TooLarge { path: String },

    #[error("buffer/* fact family is already claimed: {detail}")]
    #[diagnostic(
        code("WEAVER-BUF-004"),
        help(
            "only one weaver-buffers instance may own a given buffer entity at a time.\nStop the other instance, or open a different file."
        )
    )]
    AuthorityConflict { detail: String },
}

/// Parse the CLI, initialise tracing, and run the publisher until the
/// process is signalled or a fatal condition surfaces.
pub fn run() -> Result<(), Report> {
    let cli = Cli::parse();

    // Render the contracted version shape BEFORE any runtime setup so
    // scripted consumers get clean output from a cold binary (mirrors
    // git-watcher's F5 review fix).
    if cli.version {
        crate::version::print_version(&cli.output);
        return Ok(());
    }

    init_tracing(cli.verbose, &cli.output);

    let poll_interval = parse_duration(&cli.poll_interval).into_diagnostic()?;
    let socket = resolve_socket(cli.socket.clone());
    let paths = canonicalise_and_dedup(cli.paths.clone());
    assert!(
        !paths.is_empty(),
        "clap guarantees >=1 path unless --version"
    );

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;

    let outcome =
        runtime.block_on(async move { publisher::run(paths, socket, poll_interval).await });

    match outcome {
        Ok(()) => Ok(()),
        Err(PublisherError::BusUnavailable { .. }) => {
            eprintln!("error: {}", outcome.unwrap_err());
            std::process::exit(exit_code::BUS_UNAVAILABLE);
        }
        Err(PublisherError::AuthorityConflict { detail }) => {
            render_startup_diagnostic(StartupDiagnostic::AuthorityConflict { detail });
            std::process::exit(exit_code::AUTHORITY_CONFLICT);
        }
        Err(PublisherError::Observer { source }) => {
            render_startup_diagnostic(observer_to_diagnostic(source));
            std::process::exit(exit_code::STARTUP_FAILURE);
        }
        Err(PublisherError::Client { .. }) => {
            eprintln!("error: {}", outcome.unwrap_err());
            std::process::exit(exit_code::INTERNAL);
        }
    }
}

fn observer_to_diagnostic(err: ObserverError) -> StartupDiagnostic {
    match err {
        ObserverError::StartupFailure {
            path,
            reason,
            kind: StartupKind::NotOpenable,
        } => StartupDiagnostic::NotOpenable {
            path: path.display().to_string(),
            reason,
        },
        ObserverError::StartupFailure {
            path,
            kind: StartupKind::NotRegularFile,
            ..
        } => StartupDiagnostic::NotRegularFile {
            path: path.display().to_string(),
        },
        ObserverError::StartupFailure {
            path,
            kind: StartupKind::TooLarge,
            ..
        } => StartupDiagnostic::TooLarge {
            path: path.display().to_string(),
        },
        // Mid-session observer errors (TransientRead / Missing /
        // NotRegularFile) never leave the publisher — they drive
        // buffer/observable transitions rather than exiting. If one
        // leaks to startup classification, degrade to WEAVER-BUF-001
        // with the error's Display as the reason.
        other => StartupDiagnostic::NotOpenable {
            path: "<unknown>".into(),
            reason: other.to_string(),
        },
    }
}

fn render_startup_diagnostic(diag: StartupDiagnostic) {
    let report: Report = diag.into();
    eprintln!("{report:?}");
}

/// Parse a humantime-style duration string (e.g. `250ms`, `1s`).
///
/// `humantime` parses `0ms` as `Duration::ZERO`, which would reach
/// `tokio::time::interval` and panic. Reject zero up front so the CLI
/// fails via the documented startup-error path (exit 1).
fn parse_duration(input: &str) -> Result<Duration, CliError> {
    let parsed =
        humantime::parse_duration(input).map_err(|source| CliError::BadPollInterval { source })?;
    if parsed.is_zero() {
        return Err(CliError::ZeroPollInterval);
    }
    Ok(parsed)
}

/// Canonicalise each argv path and drop duplicates, preserving
/// first-seen order (FR-006a; Q2 clarification "at parse time").
///
/// `std::fs::canonicalize` is best-effort: when it fails (path is
/// missing, permission denied, etc.) we fall back to the raw
/// `PathBuf` as the dedup key and as the representative. That way:
///
/// - Two existing paths that canonicalise to the same absolute path
///   (`./foo` and `foo`, or `a/../b/c` and `b/c`) collapse to one
///   buffer entity, which is the FR-006a invariant.
/// - Two byte-identical raw paths that canonicalise-fail (e.g., the
///   same `/does/not/exist` passed twice) still collapse to one, so
///   only one `WEAVER-BUF-001` diagnostic surfaces downstream rather
///   than two identical copies.
/// - Distinct unresolvable paths stay distinct — each gets its own
///   `BufferState::open` attempt and its own miette diagnostic.
///
/// Emits a `debug!` line when dedup shrank the set; quiet otherwise.
fn canonicalise_and_dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let original = paths.len();
    let mut seen: HashSet<PathBuf> = HashSet::with_capacity(original);
    let mut out: Vec<PathBuf> = Vec::with_capacity(original);
    for raw in paths {
        let key = std::fs::canonicalize(&raw).unwrap_or_else(|_| raw.clone());
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    let dropped = original - out.len();
    if dropped > 0 {
        debug!(
            original,
            deduped = dropped,
            kept = out.len(),
            "collapsed duplicate CLI path(s) after canonicalisation"
        );
    }
    out
}

/// Resolve the socket path: explicit `--socket` wins, else the
/// XDG/`/tmp` default. (`WEAVER_SOCKET` env is consumed by clap's
/// `env =` attribute and surfaces as `cli.socket == Some(...)`.)
fn resolve_socket(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::Path::new(&runtime_dir).join("weaver.sock");
    }
    PathBuf::from("/tmp/weaver.sock")
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
        .unwrap_or_else(|_| EnvFilter::new(format!("weaver_buffers={default_level}")));
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

    #[test]
    fn observer_to_diagnostic_maps_each_startup_kind() {
        let p = std::path::PathBuf::from("/fixture");
        let d = observer_to_diagnostic(ObserverError::StartupFailure {
            path: p.clone(),
            reason: "missing".into(),
            kind: StartupKind::NotOpenable,
        });
        assert!(matches!(d, StartupDiagnostic::NotOpenable { .. }));

        let d = observer_to_diagnostic(ObserverError::StartupFailure {
            path: p.clone(),
            reason: "".into(),
            kind: StartupKind::NotRegularFile,
        });
        assert!(matches!(d, StartupDiagnostic::NotRegularFile { .. }));

        let d = observer_to_diagnostic(ObserverError::StartupFailure {
            path: p.clone(),
            reason: "oom".into(),
            kind: StartupKind::TooLarge,
        });
        assert!(matches!(d, StartupDiagnostic::TooLarge { .. }));
    }

    #[test]
    fn startup_diagnostic_carries_stable_codes() {
        // Guard the WEAVER-BUF-00{1,2,3,4} codes against accidental
        // rename. These are a public surface per `cli-surfaces.md §Error
        // rendering` — changing them is a CLI MAJOR.
        use miette::Diagnostic;
        for (diag, expected) in [
            (
                StartupDiagnostic::NotOpenable {
                    path: "/x".into(),
                    reason: "y".into(),
                },
                "WEAVER-BUF-001",
            ),
            (
                StartupDiagnostic::NotRegularFile { path: "/x".into() },
                "WEAVER-BUF-002",
            ),
            (
                StartupDiagnostic::TooLarge { path: "/x".into() },
                "WEAVER-BUF-003",
            ),
            (
                StartupDiagnostic::AuthorityConflict { detail: "d".into() },
                "WEAVER-BUF-004",
            ),
        ] {
            let code = diag.code().map(|c| c.to_string()).unwrap_or_default();
            assert_eq!(code, expected, "diagnostic code drift: {diag:?}");
        }
    }

    #[test]
    fn resolve_socket_prefers_explicit() {
        let p = PathBuf::from("/tmp/custom.sock");
        assert_eq!(resolve_socket(Some(p.clone())), p);
    }

    #[test]
    fn canonicalise_and_dedup_collapses_canonically_equivalent_paths() {
        use std::io::Write;
        // A real file passed via two different surface forms that
        // canonicalise to the same absolute path collapses to one
        // entry, matching FR-006a.
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(b"x").expect("write");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");
        let raw = f.path().to_path_buf();

        let out = canonicalise_and_dedup(vec![canonical.clone(), raw.clone(), canonical.clone()]);
        assert_eq!(out.len(), 1, "three equivalent paths must collapse to one");
        assert_eq!(
            out[0], canonical,
            "representative must be the canonical form"
        );
    }

    #[test]
    fn canonicalise_and_dedup_drops_byte_identical_unresolvable_duplicates() {
        // Two copies of a path that cannot be canonicalised (no such
        // file) still dedup via raw-path fallback — one diagnostic
        // downstream, not two copies of the same one.
        let missing = PathBuf::from("/definitely/not/a/real/path/weaver-dedup-test");
        let out = canonicalise_and_dedup(vec![missing.clone(), missing.clone()]);
        assert_eq!(out, vec![missing]);
    }

    #[test]
    fn canonicalise_and_dedup_preserves_distinct_unresolvable_paths() {
        // Two distinct non-existent paths each deserve their own
        // open attempt + miette diagnostic; dedup must NOT merge
        // them.
        let a = PathBuf::from("/definitely/not/a/real/path/weaver-a");
        let b = PathBuf::from("/definitely/not/a/real/path/weaver-b");
        let out = canonicalise_and_dedup(vec![a.clone(), b.clone()]);
        assert_eq!(out, vec![a, b]);
    }

    #[test]
    fn canonicalise_and_dedup_preserves_first_seen_order() {
        // Order preservation keeps the deterministic per-buffer
        // `bootstrap_tick` index in the publisher stable for a given
        // argv.
        let a = PathBuf::from("/definitely/not/a/real/path/weaver-a");
        let b = PathBuf::from("/definitely/not/a/real/path/weaver-b");
        let c = PathBuf::from("/definitely/not/a/real/path/weaver-c");
        let out =
            canonicalise_and_dedup(vec![b.clone(), a.clone(), c.clone(), a.clone(), b.clone()]);
        assert_eq!(out, vec![b, a, c]);
    }
}
