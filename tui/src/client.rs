//! TUI bus-client helpers — a thin layer over [`weaver_core::bus::client`]
//! that adds a background reader task + disconnect detection (T071).
//!
//! The reader task forwards each inbound [`BusMessage`] to an mpsc
//! channel, and an `Err`/channel-close is the disconnect signal the
//! render layer consumes (T072).

use std::path::Path;

use miette::miette;
use tokio::io::AsyncRead;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use weaver_core::bus::client::{Client, ClientError};
use weaver_core::bus::codec::{CodecError, read_message};
use weaver_core::types::message::{BusMessage, SubscribePattern};

/// Result type for the reader-task outputs. `Ok(msg)` is a live bus
/// message; `Err` is the disconnect signal (stream-level I/O error or
/// codec failure).
pub type BusStreamItem = Result<BusMessage, CodecError>;

/// A connected TUI client — the writer half plus a receiver of inbound
/// bus messages. The reader task is detached and shuts down when the
/// underlying stream closes.
pub struct TuiClient {
    pub writer: OwnedWriteHalf,
    pub inbound: mpsc::UnboundedReceiver<BusStreamItem>,
    pub reader_task: JoinHandle<()>,
}

/// Connect, handshake, subscribe to every fact family the TUI
/// renders, and spawn the background reader task.
///
/// Slice 001 shipped with `buffer/*` only. Slice 002 added `repo/*`
/// and `watcher/*` so the git-watcher's facts render in the
/// Repositories section. Slice 003 transfers `buffer/*` authority
/// from the retired `core/dirty-tracking` behavior to the new
/// `weaver-buffers` service and adds the Buffers render section
/// (T043..T045); `AllFacts` already covers the new family so no
/// subscription-level change is needed — the T043 deliverable is
/// effectively "verified as subsumed" here.
///
/// `AllFacts` is used instead of multiple prefix subscriptions —
/// the TUI cares about the full fact space for rendering and
/// inspection.
pub async fn connect(socket: &Path) -> miette::Result<TuiClient> {
    let mut client = Client::connect(socket, "tui").await.map_err(map_err)?;
    let _starting_sequence = client
        .subscribe(SubscribePattern::AllFacts)
        .await
        .map_err(map_err)?;

    let (reader, writer) = client.stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel::<BusStreamItem>();
    let reader_task = tokio::spawn(reader_loop(reader, tx));

    Ok(TuiClient {
        writer,
        inbound: rx,
        reader_task,
    })
}

async fn reader_loop<R>(mut reader: R, tx: mpsc::UnboundedSender<BusStreamItem>)
where
    R: AsyncRead + Unpin,
{
    loop {
        match read_message(&mut reader).await {
            Ok(msg) => {
                if tx.send(Ok(msg)).is_err() {
                    return;
                }
            }
            Err(e) => {
                // Single error forwarded; subsequent iterations would
                // just loop on the same error. The channel close is
                // itself the disconnect signal for the render layer.
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
}

fn map_err(e: ClientError) -> miette::Report {
    miette!("{e}").context("while connecting to the weaver bus")
}

/// Deprecated shim: earlier callers expect `connect_default`.
pub async fn connect_default(socket: &Path) -> miette::Result<TuiClient> {
    connect(socket).await
}
