//! TUI rendering — minimal text output for slice 001 Phase 2.
//!
//! Phase 2 only exercises the connection + handshake; the full
//! crossterm raw-mode loop with fact rendering lands in Phase 3
//! (T036 + T047) and stale-fact marking in T072.

use std::path::PathBuf;

use crate::client;

/// Run the TUI. Slice 001 Phase 2: connect, handshake, subscribe,
/// print a connection summary, and wait for Ctrl-C before exiting.
/// Full crossterm raw-mode + keystroke-driven commands land in Phase 3.
pub async fn run(socket: PathBuf) -> miette::Result<()> {
    println!("weaver-tui — connecting to {}", socket.display());

    match client::connect_default(&socket).await {
        Ok(c) => {
            println!(
                "  status: connected; subscribed starting at sequence {}",
                c.starting_sequence
            );
            println!(
                "  bus protocol: v{}",
                weaver_core::types::message::BUS_PROTOCOL_VERSION_STR
            );
            println!();
            println!("Facts (buffer/*): (none)");
            println!();
            println!("[Phase 2 scaffold — interactive raw-mode lands in Phase 3]");
            println!("Press Ctrl-C to exit.");

            // Drain the socket in a background task so the server side
            // doesn't block on a slow consumer. Every received message
            // is discarded for slice 001 Phase 2 (rendering lands in T047).
            let (mut reader, _writer) = tokio::io::split(c.stream);
            let drain_task = tokio::spawn(async move {
                use weaver_core::bus::codec::read_message;
                loop {
                    if read_message(&mut reader).await.is_err() {
                        break;
                    }
                }
            });

            // Wait for Ctrl-C.
            let _ = tokio::signal::ctrl_c().await;

            drain_task.abort();
            Ok(())
        }
        Err(e) => {
            println!("  status: UNAVAILABLE");
            println!("  reason: {e}");
            println!();
            println!("[Start `weaver run` in another terminal, then retry.]");
            Err(e)
        }
    }
}
