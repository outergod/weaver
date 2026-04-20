//! CLI entry points and subcommands.
//!
//! `clap` derive per L2 P6; `miette` errors with both human and JSON
//! rendering per L2 P5 / P6.

pub mod args;
pub mod config;
pub mod simulate;
pub mod tracing_setup;
pub mod version;

use clap::Parser;
use miette::IntoDiagnostic;

use args::{Cli, Command, OutputFormat};

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
        Some(Command::Status) => stub_status(output, socket),
        Some(Command::Inspect { fact_key }) => stub_inspect(output, socket, &fact_key),
        Some(Command::SimulateEdit { buffer_id }) => {
            simulate::run(simulate::SimulationKind::Edit, buffer_id, output, socket)
        }
        Some(Command::SimulateClean { buffer_id }) => {
            simulate::run(simulate::SimulationKind::Clean, buffer_id, output, socket)
        }
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
        let mut dispatcher = crate::behavior::dispatcher::Dispatcher::new();
        // Embedded behaviors (slice 001).
        dispatcher.register(Box::new(
            crate::behavior::dirty_tracking::DirtyTrackingBehavior::new(),
        ));
        let dispatcher = Arc::new(dispatcher);

        tracing::info!(
            target: "weaver::lifecycle",
            socket = %cfg.socket_path.display(),
            "started"
        );

        // Spawn the listener on the socket.
        let listener_dispatcher = Arc::clone(&dispatcher);
        let listener_socket = cfg.socket_path.clone();
        let listener_task = tokio::spawn(async move {
            if let Err(e) = crate::bus::listener::run(listener_socket, listener_dispatcher).await {
                tracing::error!(target: "weaver::bus", error = %e, "listener exited");
            }
        });

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

fn stub_status(_output: OutputFormat, _socket: Option<std::path::PathBuf>) -> miette::Result<()> {
    tracing::warn!("status subcommand: stub (real impl lands in T059, slice 001 Phase 5)");
    Ok(())
}

fn stub_inspect(
    _output: OutputFormat,
    _socket: Option<std::path::PathBuf>,
    _fact_key: &str,
) -> miette::Result<()> {
    tracing::warn!("inspect subcommand: stub (real impl lands in T054, slice 001 Phase 4)");
    Ok(())
}
