//! Bus listener — Unix-domain-socket accept loop + per-connection task.
//!
//! See `specs/001-hello-fact/contracts/bus-messages.md` for the wire
//! contract. Per-connection flow:
//!
//! 1. Handshake: expect `Hello`, validate protocol version, reply with
//!    `Lifecycle(Ready)`.
//! 2. Multiplex `tokio::select!` between inbound client messages and
//!    outbound fact events delivered by the dispatcher's fact-store
//!    subscription.
//!
//! Phase 3 extension: subscriptions are wired to the dispatcher's
//! `FactStore`, so `FactAssert` and `FactRetract` messages are forwarded
//! back to subscribers in real time (T047 + T048 depend on this).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::IntoDiagnostic;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

use crate::behavior::dispatcher::Dispatcher;
use crate::bus::codec::{CodecError, read_message, write_message};
use crate::fact_space::{FactEvent, FactStore, SubscriptionHandle};
use crate::inspect::inspect_fact;
use crate::types::message::{
    BUS_PROTOCOL_VERSION, BusMessage, ErrorMsg, HelloMsg, LifecycleSignal,
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

    #[error("protocol version mismatch: client sent {client}, core supports {core}")]
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

/// Per-connection task: handshake, then loop multiplexing inbound
/// client messages and outbound fact-space events until the stream
/// closes on either side.
async fn handle_connection(
    mut stream: UnixStream,
    dispatcher: Arc<Dispatcher>,
) -> Result<(), ListenerError> {
    // 1. Handshake: expect Hello.
    let client_kind = match read_message(&mut stream).await? {
        BusMessage::Hello(HelloMsg {
            protocol_version,
            client_kind,
        }) => {
            if protocol_version != BUS_PROTOCOL_VERSION {
                let err = BusMessage::Error(ErrorMsg {
                    category: "version-mismatch".into(),
                    detail: format!(
                        "client protocol v{protocol_version:#x}, core supports v{BUS_PROTOCOL_VERSION:#x}"
                    ),
                    context: None,
                });
                let _ = write_message(&mut stream, &err).await;
                let _ = stream.shutdown().await;
                return Err(ListenerError::VersionMismatch {
                    client: protocol_version,
                    core: BUS_PROTOCOL_VERSION,
                });
            }
            client_kind
        }
        _ => {
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "expected Hello as first message".into(),
                context: None,
            });
            let _ = write_message(&mut stream, &err).await;
            let _ = stream.shutdown().await;
            return Err(ListenerError::HandshakeNotHello);
        }
    };

    tracing::info!(target: "weaver::bus", client_kind = %client_kind, "client connected");
    write_message(&mut stream, &BusMessage::Lifecycle(LifecycleSignal::Ready)).await?;

    // 2. Multiplexed message loop.
    let mut subscription: Option<SubscriptionHandle> = None;
    loop {
        // If the client is not subscribed yet, just read from the
        // stream. Once subscribed, select between reading a client
        // frame and a fact-space event.
        let next = match subscription.as_mut() {
            Some(sub) => {
                tokio::select! {
                    msg = read_message(&mut stream) => Incoming::Client(msg),
                    evt = sub.rx.recv() => Incoming::FactEvent(evt),
                }
            }
            None => Incoming::Client(read_message(&mut stream).await),
        };

        match next {
            Incoming::Client(Ok(msg)) => {
                if let Some(new_sub) = handle_client_message(msg, &dispatcher, &mut stream).await? {
                    subscription = Some(new_sub);
                }
            }
            Incoming::Client(Err(CodecError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                tracing::info!(target: "weaver::bus", client_kind = %client_kind, "client disconnected");
                return Ok(());
            }
            Incoming::Client(Err(e)) => return Err(e.into()),
            Incoming::FactEvent(Some(evt)) => {
                forward_fact_event(evt, &mut stream).await?;
            }
            Incoming::FactEvent(None) => {
                // Subscription channel closed (should not happen in
                // slice 001 — the fact store lives as long as the
                // dispatcher). Drop the subscription and keep reading.
                subscription = None;
            }
        }
    }
}

enum Incoming {
    Client(Result<BusMessage, CodecError>),
    FactEvent(Option<FactEvent>),
}

/// Returns `Some(handle)` when a new subscription was established.
async fn handle_client_message(
    msg: BusMessage,
    dispatcher: &Arc<Dispatcher>,
    writer: &mut UnixStream,
) -> Result<Option<SubscriptionHandle>, ListenerError> {
    match msg {
        BusMessage::Hello(_) => {
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "Hello received after handshake".into(),
                context: None,
            });
            write_message(writer, &err).await?;
            Ok(None)
        }
        BusMessage::Event(event) => {
            dispatcher.process_event(event).await;
            Ok(None)
        }
        BusMessage::Subscribe(pattern) => {
            let fs = dispatcher.fact_store();
            let handle = {
                let mut fs = fs.lock().await;
                fs.subscribe(pattern)
            };
            // Starting sequence is always `0` in slice 001 — the
            // delivery layer doesn't yet stamp FactAssert/FactRetract
            // with per-publisher numbers; gap detection lands with a
            // later slice (contracts/bus-messages.md §Versioning).
            write_message(writer, &BusMessage::SubscribeAck { sequence: 0 }).await?;
            Ok(Some(handle))
        }
        BusMessage::InspectRequest { request_id, fact } => {
            let snapshot = {
                let fs = dispatcher.fact_store();
                let fs = fs.lock().await;
                fs.snapshot()
            };
            let result = {
                let trace = dispatcher.trace();
                let trace = trace.lock().await;
                inspect_fact(&snapshot, &trace, &fact)
            };
            let resp = BusMessage::InspectResponse { request_id, result };
            write_message(writer, &resp).await?;
            Ok(None)
        }
        BusMessage::StatusRequest => {
            let (lifecycle, uptime_ns, facts) = {
                let fs = dispatcher.fact_store();
                let fs = fs.lock().await;
                let snapshot = fs.snapshot();
                let facts: Vec<_> = snapshot.values().cloned().collect();
                (LifecycleSignal::Ready, dispatcher.uptime_ns(), facts)
            };
            write_message(
                writer,
                &BusMessage::StatusResponse {
                    lifecycle,
                    uptime_ns,
                    facts,
                },
            )
            .await?;
            Ok(None)
        }
        BusMessage::FactAssert(_)
        | BusMessage::FactRetract { .. }
        | BusMessage::SubscribeAck { .. }
        | BusMessage::InspectResponse { .. }
        | BusMessage::Lifecycle(_)
        | BusMessage::Error(_)
        | BusMessage::StatusResponse { .. } => {
            // These are server-originated; client should not send them.
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "client sent a server-only message kind".into(),
                context: None,
            });
            write_message(writer, &err).await?;
            Ok(None)
        }
    }
}

async fn forward_fact_event(evt: FactEvent, writer: &mut UnixStream) -> Result<(), ListenerError> {
    let msg = match evt {
        FactEvent::Asserted(fact) => BusMessage::FactAssert(fact),
        FactEvent::Retracted { key, provenance } => BusMessage::FactRetract { key, provenance },
    };
    write_message(writer, &msg).await?;
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
