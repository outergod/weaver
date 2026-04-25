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

    #[error(
        "refusing to unlink non-socket path {path:?} (file type: {kind}); \
        refusing to touch it. Use `--socket <new-path>` or remove the file manually."
    )]
    RefuseToUnlinkNonSocket { path: PathBuf, kind: &'static str },
}

/// Bind the listener to `socket_path` synchronously.
///
/// Separated from [`serve`] so `run_core` can surface bind failures
/// (missing parent directory, permission denied, path-type mismatch)
/// to the caller as documented startup errors *before* signalling
/// `Lifecycle::Ready`. Prior to this split, bind errors were swallowed
/// inside the spawned listener task and the core would happily report
/// `ready` with no bus socket bound.
pub fn bind(socket_path: &Path) -> miette::Result<UnixListener> {
    // Remove a stale socket file from a previous run, if present — but
    // ONLY if the path actually holds a Unix-domain socket. Blindly
    // unlinking whatever the caller pointed `--socket` at would happily
    // delete a regular file (e.g., if a user typo'd `weaver run
    // --socket /etc/passwd`). Defense in depth against caller error.
    if let Some(kind) = classify_path_to_unlink(socket_path).into_diagnostic()? {
        if kind == "socket" {
            std::fs::remove_file(socket_path).into_diagnostic()?;
        } else {
            return Err(ListenerError::RefuseToUnlinkNonSocket {
                path: socket_path.to_path_buf(),
                kind,
            })
            .into_diagnostic();
        }
    }

    let listener = UnixListener::bind(socket_path).into_diagnostic()?;
    tracing::info!(target: "weaver::bus", path = %socket_path.display(), "listening");
    Ok(listener)
}

/// Run the accept loop against an already-bound listener. Accepts
/// connections until the task is aborted; each connection runs in its
/// own sub-task.
pub async fn serve(listener: UnixListener, dispatcher: Arc<Dispatcher>) {
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

/// Monotonic connection-id counter used by the authority-conflict
/// mechanism (FR-009). Each handled connection gets a unique id so
/// authority claims can be released on disconnect.
static CONN_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Per-connection task: handshake, then loop multiplexing inbound
/// client messages and outbound fact-space events until the stream
/// closes on either side.
async fn handle_connection(
    mut stream: UnixStream,
    dispatcher: Arc<Dispatcher>,
) -> Result<(), ListenerError> {
    let conn_id = CONN_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                        "bus protocol {BUS_PROTOCOL_VERSION:#04x} required; received {protocol_version:#04x}"
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

    // F7 review fix: every exit from the post-handshake loop — clean
    // EOF, codec error reading, a write failure bubbled up from
    // `handle_client_message`, or a subscription forward failure —
    // must release this connection's claims + conn-owned facts.
    // Earlier the `?` propagation on write-side errors skipped
    // cleanup entirely, so a client that both published and subscribed
    // would leak its authority claims on broken-pipe, blocking
    // replacement publishers until core restart. Funneling the loop
    // through an inner helper guarantees the cleanup always runs.
    let result = run_message_loop(conn_id, &mut stream, &dispatcher, &client_kind).await;
    dispatcher.release_connection(conn_id).await;
    result
}

async fn run_message_loop(
    conn_id: u64,
    stream: &mut UnixStream,
    dispatcher: &Arc<Dispatcher>,
    client_kind: &str,
) -> Result<(), ListenerError> {
    let mut subscription: Option<SubscriptionHandle> = None;
    loop {
        // If the client is not subscribed yet, just read from the
        // stream. Once subscribed, select between reading a client
        // frame and a fact-space event.
        let next = match subscription.as_mut() {
            Some(sub) => {
                tokio::select! {
                    msg = read_message(stream) => Incoming::Client(msg),
                    evt = sub.rx.recv() => Incoming::FactEvent(evt),
                }
            }
            None => Incoming::Client(read_message(stream).await),
        };

        match next {
            Incoming::Client(Ok(msg)) => {
                if let Some(new_sub) =
                    handle_client_message(conn_id, msg, dispatcher, stream).await?
                {
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
                forward_fact_event(evt, stream).await?;
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
    conn_id: u64,
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
            // F15 review fix: Events carry client-supplied provenance
            // and land in the trace unchanged via `process_event`.
            // Without a structural check here a deserialized
            // `ActorIdentity::Service` with an empty or non-kebab
            // `service_id` would poison inspection output. Validate
            // before dispatch — same error shape as the FactAssert
            // path (F12) so clients get a consistent diagnostic.
            if let Err(e) = event.provenance.source.validate() {
                let err = BusMessage::Error(ErrorMsg {
                    category: "invalid-identity".into(),
                    detail: format!("event provenance rejected: {e}"),
                    context: None,
                });
                write_message(writer, &err).await?;
                return Ok(None);
            }
            dispatcher.process_event(event).await;
            Ok(None)
        }
        BusMessage::Subscribe(pattern) => {
            // Take the snapshot and register the subscription under the
            // same fact-store lock so the two are atomic with respect
            // to the broadcast path. Any FactAssert published *after*
            // our lock release reaches the handle via the mpsc; anything
            // before is covered by the snapshot we're about to replay.
            let fs = dispatcher.fact_store();
            let (snapshot, handle) = {
                let mut fs = fs.lock().await;
                let snap = fs.snapshot();
                let handle = fs.subscribe(pattern.clone());
                (snap, handle)
            };
            // Starting sequence is always `0` in slice 001 — the
            // delivery layer doesn't yet stamp FactAssert/FactRetract
            // with per-publisher numbers; gap detection lands with a
            // later slice (contracts/bus-messages.md §Versioning).
            write_message(writer, &BusMessage::SubscribeAck { sequence: 0 }).await?;
            // Snapshot on subscribe — emit FactAssert for every
            // currently-asserted fact that matches the pattern, per
            // `contracts/bus-messages.md` §FactAssert ("on reconnect,
            // subscribers receive the current snapshot of subscribed
            // fact families followed by missed deltas"). Without this,
            // a client that subscribes AFTER a fact was asserted would
            // never learn the current state.
            for fact in snapshot.values() {
                if pattern.matches(&fact.key) {
                    write_message(writer, &BusMessage::FactAssert(fact.clone())).await?;
                }
            }
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
        BusMessage::FactAssert(fact) => {
            // Slice 002: only services publish authoritative facts
            // over the bus. Behaviors publish via the in-process
            // dispatcher; core asserts its own lifecycle facts
            // directly. Reject any other provenance up front —
            // otherwise a client could impersonate a behavior or
            // write into families (e.g. `buffer/*`) that core or
            // behaviors own, bypassing the single-writer rule
            // (F8 review fix).
            //
            // FR-009: first claim wins per (family, entity); a second
            // actor asserting into the same pair receives a structured
            // `authority-conflict` error.
            use crate::behavior::dispatcher::ServicePublishOutcome;
            use crate::provenance::ActorIdentity;
            if !matches!(fact.provenance.source, ActorIdentity::Service { .. }) {
                let err = BusMessage::Error(ErrorMsg {
                    category: "unauthorized".into(),
                    detail: format!(
                        "bus FactAssert requires ActorIdentity::Service provenance; got {}",
                        fact.provenance.source.kind_label(),
                    ),
                    context: None,
                });
                write_message(writer, &err).await?;
                return Ok(None);
            }
            // F12 review fix: wire deserialization bypasses
            // `ActorIdentity::service`'s kebab-case/non-empty
            // check, so a malformed `service_id` on the wire
            // would reach the trace + authority map unaltered.
            // Revalidate here — the constructor path was already
            // safe via `Provenance::new`.
            if let Err(e) = fact.provenance.source.validate() {
                let err = BusMessage::Error(ErrorMsg {
                    category: "invalid-identity".into(),
                    detail: format!("service identity rejected: {e}"),
                    context: None,
                });
                write_message(writer, &err).await?;
                return Ok(None);
            }
            match dispatcher.publish_from_service(conn_id, fact).await {
                ServicePublishOutcome::Asserted => {}
                ServicePublishOutcome::AuthorityConflict {
                    family,
                    entity,
                    existing,
                } => {
                    let detail = format!(
                        "{family}/* for entity {} already claimed by {}",
                        entity.as_u64(),
                        existing.identifying_label(),
                    );
                    let err = BusMessage::Error(ErrorMsg {
                        category: "authority-conflict".into(),
                        detail,
                        context: None,
                    });
                    write_message(writer, &err).await?;
                }
                ServicePublishOutcome::IdentityDrift { bound, attempted } => {
                    // F14: this connection already published under a
                    // different identity; refuse to let the second
                    // attribution silently overwrite the first. Detail
                    // renders via `identifying_label` so an operator
                    // diagnosing drift sees WHICH service-id the
                    // connection bound to and WHICH it tried to
                    // impersonate — kind labels alone ("bound to
                    // service; refusing FactAssert as service") leave
                    // no forensic signal.
                    let err = BusMessage::Error(ErrorMsg {
                        category: "identity-drift".into(),
                        detail: format!(
                            "connection bound to {}; refusing FactAssert as {}",
                            bound.identifying_label(),
                            attempted.identifying_label(),
                        ),
                        context: None,
                    });
                    write_message(writer, &err).await?;
                }
            }
            Ok(None)
        }
        BusMessage::FactRetract { key, provenance } => {
            // F2 review fix: a connection may only retract facts it
            // previously asserted. The dispatcher checks ownership
            // and returns NotOwned when another actor holds the
            // claim; we surface that as a structured bus Error so
            // the offending client can distinguish this from a
            // silent idempotent no-op (`NotPresent`).
            //
            // F11 review fix: the client-supplied `provenance.source`
            // and `.timestamp_ns` are intentionally ignored. The
            // dispatcher synthesizes retraction attribution server-
            // side from the asserting actor's stored identity;
            // accepting the client's source would let an owner forge
            // trace/audit attribution (e.g. retract while claiming
            // to be `ActorIdentity::Core`). The `causal_parent`
            // survives as a correlation hint so consumers can still
            // group a retract+assert pair describing one transition
            // (L2 P11).
            use crate::behavior::dispatcher::ServiceRetractOutcome;
            let outcome = dispatcher
                .retract_from_service(conn_id, key.clone(), provenance.causal_parent)
                .await;
            if matches!(outcome, ServiceRetractOutcome::NotOwned) {
                let err = BusMessage::Error(ErrorMsg {
                    category: "not-owner".into(),
                    detail: format!(
                        "cannot retract fact ({}, {}): claim held by a different connection",
                        key.entity.as_u64(),
                        key.attribute,
                    ),
                    context: None,
                });
                write_message(writer, &err).await?;
            }
            Ok(None)
        }
        BusMessage::SubscribeAck { .. }
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

/// Inspect `path` to decide whether pre-bind cleanup should touch it.
///
/// * `Ok(None)` — path does not exist; nothing to unlink.
/// * `Ok(Some("socket"))` — path is a Unix-domain socket; safe to unlink.
/// * `Ok(Some(other))` — any other file type (regular file, directory,
///   symlink, fifo, block/char device); the caller must refuse rather
///   than destroy user data.
/// * `Err(...)` — stat failed with an error other than `NotFound`.
///
/// Uses `symlink_metadata` so a symlink pointing at a socket is
/// reported as `"symlink"` rather than `"socket"` — following the
/// link and then unlinking would remove the symlink itself, but the
/// principle of least surprise is to refuse when the caller's path
/// doesn't directly name a socket.
fn classify_path_to_unlink(path: &Path) -> std::io::Result<Option<&'static str>> {
    use std::os::unix::fs::FileTypeExt;

    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let ft = meta.file_type();
    let kind = if ft.is_socket() {
        "socket"
    } else if ft.is_symlink() {
        "symlink"
    } else if ft.is_dir() {
        "directory"
    } else if ft.is_file() {
        "regular file"
    } else if ft.is_fifo() {
        "fifo"
    } else if ft.is_block_device() {
        "block-device"
    } else if ft.is_char_device() {
        "char-device"
    } else {
        "unknown"
    };
    Ok(Some(kind))
}

/// Convenience helper used by tests.
pub fn is_socket(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod classify_tests {
    use super::classify_path_to_unlink;
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::net::UnixListener as StdUnixListener;

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "weaver-classify-{tag}-{pid}-{nanos}",
            pid = std::process::id(),
        ))
    }

    #[test]
    fn missing_path_returns_none() {
        let p = unique_tmp("missing");
        assert_eq!(classify_path_to_unlink(&p).unwrap(), None);
    }

    #[test]
    fn regular_file_returns_file_kind() {
        let p = unique_tmp("regular");
        let mut f = File::create(&p).unwrap();
        f.write_all(b"sensitive").unwrap();
        assert_eq!(classify_path_to_unlink(&p).unwrap(), Some("regular file"));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn directory_returns_directory_kind() {
        let p = unique_tmp("directory");
        std::fs::create_dir(&p).unwrap();
        assert_eq!(classify_path_to_unlink(&p).unwrap(), Some("directory"));
        std::fs::remove_dir(&p).unwrap();
    }

    #[test]
    fn unix_socket_returns_socket_kind() {
        let p = unique_tmp("socket");
        let _listener = StdUnixListener::bind(&p).unwrap();
        assert_eq!(classify_path_to_unlink(&p).unwrap(), Some("socket"));
        std::fs::remove_file(&p).unwrap();
    }
}

#[cfg(test)]
mod handshake_tests {
    use super::*;
    use crate::bus::codec::{read_message, write_message};
    use crate::types::message::{BusMessage, HelloMsg};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn mismatched_hello_is_rejected_with_contract_detail() {
        let (server, mut client) = UnixStream::pair().expect("pair");
        let dispatcher = Arc::new(Dispatcher::new());

        let server_task = tokio::spawn(handle_connection(server, dispatcher));

        // Client announces the prior protocol version. The contract
        // (specs/004-buffer-edit/contracts/bus-messages.md §Connection
        // lifecycle) pins the exact `detail` wording the core must
        // emit so operators see a consistent diagnostic.
        let stale_version: u8 = 0x03;
        write_message(
            &mut client,
            &BusMessage::Hello(HelloMsg {
                protocol_version: stale_version,
                client_kind: "test".into(),
            }),
        )
        .await
        .expect("write Hello");

        let response = read_message(&mut client).await.expect("read Error");
        match response {
            BusMessage::Error(err) => {
                assert_eq!(err.category, "version-mismatch");
                assert_eq!(
                    err.detail,
                    format!(
                        "bus protocol {BUS_PROTOCOL_VERSION:#04x} required; received {stale_version:#04x}"
                    ),
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // The listener returns VersionMismatch; `serve` would log this,
        // but for the unit test we just confirm the task terminates
        // promptly rather than hanging.
        let outcome = server_task.await.expect("server task joins");
        assert!(matches!(
            outcome,
            Err(ListenerError::VersionMismatch { .. })
        ));
    }
}
