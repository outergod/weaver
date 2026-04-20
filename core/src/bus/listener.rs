//! Bus listener — Unix-domain-socket accept loop + per-connection task.
//!
//! See `specs/001-hello-fact/contracts/bus-messages.md` for the wire
//! contract. Slice 001 Phase 2 ships the handshake + simple message
//! dispatch; richer subscription forwarding and inspection lands with
//! the dispatcher integration in Phase 3 / Phase 4.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::IntoDiagnostic;
use thiserror::Error;
use tokio::io::{AsyncWriteExt, split};
use tokio::net::{UnixListener, UnixStream};

use crate::behavior::dispatcher::Dispatcher;
use crate::bus::codec::{CodecError, read_message, write_message};
use crate::types::message::{
    BUS_PROTOCOL_VERSION, BusMessage, ErrorMsg, HelloMsg, InspectionError, LifecycleSignal,
};

/// Error type surfaced by the listener.
#[derive(Debug, Error)]
pub enum ListenerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("client sent non-Hello message as first frame")]
    HandshakeNotHello,

    #[error(
        "protocol version mismatch: client sent {client}, core supports {core}"
    )]
    VersionMismatch { client: u8, core: u8 },
}

/// Start the bus listener on the given socket path. Removes a stale
/// socket file at the path before binding. Accepts connections in a
/// loop until the process is terminated; each connection runs in its
/// own task.
pub async fn run(socket_path: PathBuf, dispatcher: Arc<Dispatcher>) -> miette::Result<()> {
    // Remove a stale socket file from a previous run, if present.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).into_diagnostic()?;
    }

    let listener = UnixListener::bind(&socket_path).into_diagnostic()?;
    tracing::info!(target: "weaver::bus", path = %socket_path.display(), "listening");

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(target: "weaver::bus", error = %e, "accept failed");
                continue;
            }
        };
        let dispatcher = Arc::clone(&dispatcher);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, dispatcher).await {
                tracing::warn!(target: "weaver::bus", error = %e, "connection ended with error");
            }
        });
    }
}

/// Per-connection task: handshake, then loop on inbound messages until
/// the stream closes.
async fn handle_connection(
    stream: UnixStream,
    dispatcher: Arc<Dispatcher>,
) -> Result<(), ListenerError> {
    let (reader, writer) = split(stream);
    let mut reader = reader;
    let mut writer = writer;

    // 1. Handshake: expect Hello.
    let first = read_message(&mut reader).await?;
    let BusMessage::Hello(HelloMsg {
        protocol_version,
        client_kind,
    }) = first
    else {
        let err = BusMessage::Error(ErrorMsg {
            category: "protocol".into(),
            detail: "expected Hello as first message".into(),
            context: None,
        });
        let _ = write_message(&mut writer, &err).await;
        let _ = writer.shutdown().await;
        return Err(ListenerError::HandshakeNotHello);
    };
    if protocol_version != BUS_PROTOCOL_VERSION {
        let err = BusMessage::Error(ErrorMsg {
            category: "version-mismatch".into(),
            detail: format!(
                "client protocol v{protocol_version:#x}, core supports v{BUS_PROTOCOL_VERSION:#x}"
            ),
            context: None,
        });
        let _ = write_message(&mut writer, &err).await;
        let _ = writer.shutdown().await;
        return Err(ListenerError::VersionMismatch {
            client: protocol_version,
            core: BUS_PROTOCOL_VERSION,
        });
    }

    tracing::info!(target: "weaver::bus", client_kind = %client_kind, "client connected");
    write_message(&mut writer, &BusMessage::Lifecycle(LifecycleSignal::Ready)).await?;

    // 2. Message loop.
    loop {
        let msg = match read_message(&mut reader).await {
            Ok(m) => m,
            Err(CodecError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::info!(target: "weaver::bus", client_kind = %client_kind, "client disconnected");
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        handle_message(msg, &dispatcher, &mut writer).await?;
    }
}

async fn handle_message<W>(
    msg: BusMessage,
    dispatcher: &Arc<Dispatcher>,
    writer: &mut W,
) -> Result<(), ListenerError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    match msg {
        BusMessage::Hello(_) => {
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "Hello received after handshake".into(),
                context: None,
            });
            write_message(writer, &err).await?;
        }
        BusMessage::Event(event) => {
            dispatcher.process_event(event).await;
        }
        BusMessage::Subscribe(_pattern) => {
            // Slice 001 Phase 2: ack the subscription but don't forward
            // fact events back yet. Real subscription forwarding lands
            // with the dispatcher integration (Phase 3) once behaviors
            // produce facts to forward.
            write_message(writer, &BusMessage::SubscribeAck { sequence: 0 }).await?;
        }
        BusMessage::InspectRequest { request_id, fact: _ } => {
            // Slice 001 Phase 2: no facts asserted yet, so inspection
            // always returns FactNotFound. The real handler lands in T052.
            let resp = BusMessage::InspectResponse {
                request_id,
                result: Err(InspectionError::FactNotFound),
            };
            write_message(writer, &resp).await?;
        }
        BusMessage::FactAssert(_)
        | BusMessage::FactRetract { .. }
        | BusMessage::SubscribeAck { .. }
        | BusMessage::InspectResponse { .. }
        | BusMessage::Lifecycle(_)
        | BusMessage::Error(_) => {
            // These are server-originated; client should not send them.
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "client sent a server-only message kind".into(),
                context: None,
            });
            write_message(writer, &err).await?;
        }
    }
    Ok(())
}

/// Produce the default socket path per `cli::config::Config`.
pub fn default_socket_path() -> PathBuf {
    crate::cli::config::Config::default_socket_path()
}

/// Convenience helper used by tests.
pub fn is_socket(path: &Path) -> bool {
    path.exists()
}
