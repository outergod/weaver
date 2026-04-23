//! CLI entry points and subcommands.
//!
//! `clap` derive per L2 P6; `miette` errors with both human and JSON
//! rendering per L2 P5 / P6.

pub mod args;
pub mod config;
pub mod errors;
pub mod inspect;
pub mod output;
pub mod status;
pub mod tracing_setup;
pub mod version;

use clap::Parser;
use miette::IntoDiagnostic;

use args::{Cli, Command};

/// Process entry point — invoked from `core/src/main.rs`.
pub fn run() -> miette::Result<()> {
    let cli = Cli::parse();
    tracing_setup::init(cli.verbose);

    if cli.version {
        version::print_version(cli.output);
        return Ok(());
    }

    let output = cli.output;
    let socket = cli.socket.clone();
    match cli.command {
        Some(Command::Run) => run_core(socket),
        Some(Command::Status) => status::run(output, socket),
        Some(Command::Inspect { fact_key }) => inspect::run(&fact_key, output, socket),
        None => {
            print_help();
            Ok(())
        }
    }
}

fn print_help() {
    let mut cmd = <Cli as clap::CommandFactory>::command();
    cmd.print_help().ok();
    println!();
}

fn run_core(socket_override: Option<std::path::PathBuf>) -> miette::Result<()> {
    use std::sync::Arc;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        let cfg = config::Config::from_cli(socket_override);
        // Slice 003: the shipped core registers no embedded behaviors.
        // `buffer/dirty` authority moved to the `weaver-buffers`
        // service; any future embedded behavior lands under its own
        // registration call here.
        let dispatcher = crate::behavior::dispatcher::Dispatcher::new();
        let dispatcher = Arc::new(dispatcher);

        tracing::info!(
            target: "weaver::lifecycle",
            socket = %cfg.socket_path.display(),
            "started"
        );

        // Bind the listener synchronously so startup errors (missing
        // parent directory, permission denied, non-socket path) surface
        // as a non-zero exit before we signal `ready`. A prior
        // implementation spawned the bind inside a detached task and
        // only logged on failure; `run_core` would then hang in the
        // signal loop with no bus, masking the actual error.
        let listener = crate::bus::listener::bind(&cfg.socket_path)?;
        let listener_dispatcher = Arc::clone(&dispatcher);
        let listener_task =
            tokio::spawn(crate::bus::listener::serve(listener, listener_dispatcher));

        tracing::info!(target: "weaver::lifecycle", "ready");

        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .into_diagnostic()?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .into_diagnostic()?;
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }

        listener_task.abort();
        // Clean up the socket file for the next run.
        let _ = std::fs::remove_file(&cfg.socket_path);

        tracing::info!(target: "weaver::lifecycle", "stopped");
        Ok::<_, miette::Report>(())
    })
}
