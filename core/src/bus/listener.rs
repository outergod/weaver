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
use crate::bus::event_subscriptions::EventSubscriptionHandle;
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
    let mut fact_subscription: Option<SubscriptionHandle> = None;
    let mut event_subscription: Option<EventSubscriptionHandle> = None;
    loop {
        // The select! shape depends on which subscriptions are active
        // (a connection MAY hold one, both, or neither). Each branch
        // races client-stream reads against the active subscription
        // channels; events and fact-events are independent streams,
        // so we expand the cartesian-product cases inline rather than
        // building a multi-source merger.
        let next = match (fact_subscription.as_mut(), event_subscription.as_mut()) {
            (Some(fs), Some(es)) => tokio::select! {
                msg = read_message(stream) => Incoming::Client(msg),
                evt = fs.rx.recv() => Incoming::FactEvent(evt),
                evt = es.rx.recv() => Incoming::Event(evt),
            },
            (Some(fs), None) => tokio::select! {
                msg = read_message(stream) => Incoming::Client(msg),
                evt = fs.rx.recv() => Incoming::FactEvent(evt),
            },
            (None, Some(es)) => tokio::select! {
                msg = read_message(stream) => Incoming::Client(msg),
                evt = es.rx.recv() => Incoming::Event(evt),
            },
            (None, None) => Incoming::Client(read_message(stream).await),
        };

        match next {
            Incoming::Client(Ok(msg)) => {
                match handle_client_message(conn_id, msg, dispatcher, stream).await? {
                    HandlerOutcome::None => {}
                    HandlerOutcome::FactSubscription(h) => {
                        fact_subscription = Some(h);
                    }
                    HandlerOutcome::EventSubscription(h) => {
                        // Last-wins per the SubscribeEvents contract:
                        // overwriting drops the prior handle, whose
                        // tx side is then closed; the registry prunes
                        // it on the next broadcast.
                        event_subscription = Some(h);
                    }
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
                fact_subscription = None;
            }
            Incoming::Event(Some(event)) => {
                write_message(stream, &BusMessage::Event(event)).await?;
            }
            Incoming::Event(None) => {
                // Event-subscription channel closed. Same defensive
                // shape as the fact-subscription None branch.
                event_subscription = None;
            }
        }
    }
}

enum Incoming {
    Client(Result<BusMessage, CodecError>),
    FactEvent(Option<FactEvent>),
    Event(Option<crate::types::event::Event>),
}

/// Outcome of handling one client message. The dispatch loop folds
/// subscription handles into its connection-local state; everything
/// else collapses to `None`.
enum HandlerOutcome {
    None,
    FactSubscription(SubscriptionHandle),
    EventSubscription(EventSubscriptionHandle),
}

async fn handle_client_message(
    conn_id: u64,
    msg: BusMessage,
    dispatcher: &Arc<Dispatcher>,
    writer: &mut UnixStream,
) -> Result<HandlerOutcome, ListenerError> {
    match msg {
        BusMessage::Hello(_) => {
            let err = BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "Hello received after handshake".into(),
                context: None,
            });
            write_message(writer, &err).await?;
            Ok(HandlerOutcome::None)
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
                return Ok(HandlerOutcome::None);
            }
            // Slice 004 review: structural envelope checks. Reject
            // `EventId::ZERO` (reserved sentinel — see §28) and
            // BufferEdit target/payload entity mismatch (corrupts
            // trace + inspect-why attribution — see comment on
            // `validate_event_envelope`). Same rejection shape as the
            // identity check above.
            if let Err(e) = validate_event_envelope(&event) {
                let err = BusMessage::Error(ErrorMsg {
                    category: "invalid-event-envelope".into(),
                    detail: e,
                    context: None,
                });
                write_message(writer, &err).await?;
                return Ok(HandlerOutcome::None);
            }
            dispatcher.process_event(event).await;
            Ok(HandlerOutcome::None)
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
            Ok(HandlerOutcome::FactSubscription(handle))
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
            Ok(HandlerOutcome::None)
        }
        BusMessage::EventInspectRequest {
            request_id,
            event_id,
        } => {
            // Slice 004 — see specs/004-buffer-edit/research.md §14.
            let result = lookup_event_for_inspect(dispatcher, event_id).await;
            let resp = BusMessage::EventInspectResponse { request_id, result };
            write_message(writer, &resp).await?;
            Ok(HandlerOutcome::None)
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
            Ok(HandlerOutcome::None)
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
                return Ok(HandlerOutcome::None);
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
                return Ok(HandlerOutcome::None);
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
            Ok(HandlerOutcome::None)
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
            Ok(HandlerOutcome::None)
        }
        BusMessage::SubscribeAck { .. }
        | BusMessage::InspectResponse { .. }
        | BusMessage::EventInspectResponse { .. }
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
            Ok(HandlerOutcome::None)
        }
        BusMessage::SubscribeEvents(pattern) => {
            // Slice 004: register an event subscription on the
            // dispatcher's broadcast registry. Unlike fact-Subscribe
            // there is no snapshot — events are lossy-class with no
            // replay (`docs/02-architecture.md §3.1`); subscribers
            // see only events that arrive AFTER subscription. The
            // ack reuses SubscribeAck { sequence: 0 } per the
            // bus-messages contract (events have no per-publisher
            // sequence; sequence=0 is consistent with the slice-001
            // fact ack which also uses 0 until gap detection lands).
            let handle = dispatcher.event_subscriptions().subscribe(pattern);
            write_message(writer, &BusMessage::SubscribeAck { sequence: 0 }).await?;
            Ok(HandlerOutcome::EventSubscription(handle))
        }
    }
}

/// Slice-004 `EventInspectRequest` lookup. Returns the `Event` envelope
/// at `event_id` or `EventNotFound`.
///
/// `EventId` is unique per producer, not globally — concurrent producers
/// minting the same id leave only the latest event in the trace's
/// `by_event` index, so this walkback may attribute a fact to the wrong
/// emitter on collision. Class-wide fix tracked at
/// `docs/07-open-questions.md §28`.
///
/// Short-circuits [`EventId::ZERO`] to `EventNotFound`: the
/// inspect-handler uses ZERO as a sentinel for "fact has no
/// causal_parent" (`core/src/inspect/handler.rs:140`), so a walkback
/// request carrying ZERO must produce a clean miss rather than
/// resolving to whichever real event happens to sit at id 0 in the
/// `by_event` index. Slice 004 closed the deterministic-collision case
/// (`weaver-buffers` bootstrap formerly minted `EventId::new(idx)`
/// starting at 0; now wall-clock-based via `now_ns().wrapping_add(idx)`).
///
/// **Frame-size headroom asymmetry** (`docs/07-open-questions.md §29`):
/// the caller's `BusMessage::EventInspectResponse` wrapper around the
/// returned `Event` can hit `CodecError::FrameTooLarge` if a non-CLI
/// producer ingested an event sized in
/// `(MAX_EVENT_INGEST_FRAME, MAX_FRAME_SIZE]`. CLI emitters enforce
/// the smaller limit at dispatch (`core/src/cli/edit.rs::send_event_with_ingest_check`),
/// so no current production producer can trigger this; the listener
/// stays unguarded pending a future producer landscape change (see §29
/// candidate resolutions).
async fn lookup_event_for_inspect(
    dispatcher: &Dispatcher,
    event_id: crate::types::ids::EventId,
) -> Result<crate::types::event::Event, crate::types::message::EventInspectionError> {
    use crate::types::ids::EventId;
    use crate::types::message::EventInspectionError;
    if event_id == EventId::ZERO {
        return Err(EventInspectionError::EventNotFound);
    }
    let trace = dispatcher.trace();
    let trace = trace.lock().await;
    trace
        .find_event(event_id)
        .and_then(|seq| trace.get(seq))
        .and_then(|entry| match &entry.payload {
            crate::trace::entry::TracePayload::Event { event } => Some(event.clone()),
            // Defensive: find_event's index should only point at Event
            // payloads; any other payload is a structural bug in
            // TraceStore::update_indexes. Treat as EventNotFound rather
            // than panic — the chain walk surfaces the symptom cleanly
            // without crashing the listener.
            _ => None,
        })
        .ok_or(EventInspectionError::EventNotFound)
}

/// Reject malformed `BusMessage::Event` envelopes at the listener
/// boundary. Symmetric with the F15 [`crate::provenance::ActorIdentity`]
/// `validate` check on the same code path.
///
/// Two structural invariants enforced today:
///
/// * **`EventId::ZERO` is reserved.** The inspect handler uses
///   [`crate::types::ids::EventId::ZERO`] as a sentinel for "fact has no
///   `causal_parent`" (`core/src/inspect/handler.rs:140`), and
///   [`lookup_event_for_inspect`] short-circuits ZERO requests to
///   `EventNotFound` to keep that sentinel meaning intact. A real event
///   ingested with ID 0 would therefore become uninspectable; reject
///   at ingest so non-CLI producers can't silently lose provenance.
///   Closes one §28 concrete instance at the producer side
///   (`docs/07-open-questions.md §28`); the lookup short-circuit
///   becomes belt-and-braces.
/// * **`BufferEdit` envelope/payload entity must agree.** The dispatcher
///   forwards `event.target` to the trace unchanged; `weaver-buffers`
///   dispatches on `payload.entity`. A producer that sets these to
///   different entities (CLI emitters always set them from the same
///   variable, but non-CLI producers can drift) would apply edits to
///   one buffer while attributing the source event to another in
///   `weaver inspect --why`. Reject at ingest.
fn validate_event_envelope(event: &crate::types::event::Event) -> Result<(), String> {
    use crate::types::event::EventPayload;
    use crate::types::ids::EventId;

    if event.id == EventId::ZERO {
        return Err(
            "EventId::ZERO is reserved as the 'no causal_parent' sentinel; \
             producers must mint a non-zero id (per docs/07-open-questions.md §28)"
                .to_string(),
        );
    }
    match &event.payload {
        EventPayload::BufferEdit { entity, .. } => match event.target {
            Some(target) if target == *entity => {}
            other => {
                return Err(format!(
                    "BufferEdit envelope/payload mismatch: target={} payload.entity={}",
                    other.map_or("None".to_string(), |e| e.as_u64().to_string()),
                    entity.as_u64()
                ));
            }
        },
        EventPayload::BufferOpen { .. } => {
            // No entity in payload to compare; weaver-buffers re-derives
            // the canonical entity server-side from the path.
        }
    }
    Ok(())
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
        // Client announces the immediate-prior protocol version. The
        // contract (specs/005-buffer-save/contracts/bus-messages.md
        // §Connection lifecycle) pins the exact `detail` wording the
        // core must emit so operators see a consistent diagnostic.
        assert_version_rejected_with_contract_detail(0x04).await;
    }

    #[tokio::test]
    async fn forward_incompatible_hellos_are_rejected_with_contract_detail() {
        // Forward-incompatibility check: every protocol version older
        // than the immediate-prior must also reject with the same
        // category and detail-string template, so a long-tail v0.1/v0.2
        // client gets the same diagnostic shape as a v0.4 one.
        for stale_version in [0x01u8, 0x02, 0x03] {
            assert_version_rejected_with_contract_detail(stale_version).await;
        }
    }

    async fn assert_version_rejected_with_contract_detail(stale_version: u8) {
        let (server, mut client) = UnixStream::pair().expect("pair");
        let dispatcher = Arc::new(Dispatcher::new());

        let server_task = tokio::spawn(handle_connection(server, dispatcher));

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

#[cfg(test)]
mod event_inspect_lookup_tests {
    use super::*;
    use crate::provenance::{ActorIdentity, Provenance};
    use crate::trace::entry::TracePayload;
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::{Event, EventPayload};
    use crate::types::ids::EventId;
    use crate::types::message::EventInspectionError;

    fn fixture_event(id: EventId) -> Event {
        Event {
            id,
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Core, 0, None).unwrap(),
        }
    }

    /// Regression for the slice-004 finding: even when a real event
    /// with `EventId(0)` is in the trace, an `EventInspectRequest`
    /// carrying `EventId::ZERO` must short-circuit to `EventNotFound`.
    /// The inspect-handler treats ZERO as the "no causal_parent"
    /// sentinel; resolving it to a real event would mis-attribute facts
    /// that have no source event.
    #[tokio::test]
    async fn lookup_event_for_inspect_short_circuits_zero_even_when_event_zero_exists() {
        let dispatcher = Dispatcher::new();
        // Inject an event at id 0 directly into the trace (simulates a
        // pre-fix bootstrap_tick collision with the ZERO sentinel).
        {
            let trace_arc = dispatcher.trace();
            let mut trace = trace_arc.lock().await;
            trace.append(
                0,
                TracePayload::Event {
                    event: fixture_event(EventId::ZERO),
                },
            );
        }
        let result = lookup_event_for_inspect(&dispatcher, EventId::ZERO).await;
        assert_eq!(
            result,
            Err(EventInspectionError::EventNotFound),
            "ZERO must short-circuit to EventNotFound regardless of trace contents",
        );
    }

    #[tokio::test]
    async fn lookup_event_for_inspect_returns_event_for_real_id() {
        let dispatcher = Dispatcher::new();
        let id = EventId::new(42);
        {
            let trace_arc = dispatcher.trace();
            let mut trace = trace_arc.lock().await;
            trace.append(
                0,
                TracePayload::Event {
                    event: fixture_event(id),
                },
            );
        }
        let got = lookup_event_for_inspect(&dispatcher, id)
            .await
            .expect("lookup hits the trace");
        assert_eq!(got.id, id);
    }

    #[tokio::test]
    async fn lookup_event_for_inspect_missing_id_returns_event_not_found() {
        let dispatcher = Dispatcher::new();
        let result = lookup_event_for_inspect(&dispatcher, EventId::new(123)).await;
        assert_eq!(result, Err(EventInspectionError::EventNotFound));
    }
}

#[cfg(test)]
mod event_envelope_validation_tests {
    use super::*;
    use crate::provenance::{ActorIdentity, Provenance};
    use crate::types::edit::{Position, Range, TextEdit};
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::{Event, EventPayload};
    use crate::types::ids::EventId;

    fn buffer_edit(id: EventId, target: Option<EntityRef>, payload_entity: EntityRef) -> Event {
        Event {
            id,
            name: "buffer/edit".into(),
            target,
            payload: EventPayload::BufferEdit {
                entity: payload_entity,
                version: 0,
                edits: vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                        },
                    },
                    new_text: "x".into(),
                }],
            },
            provenance: Provenance::new(ActorIdentity::User, 0, None).unwrap(),
        }
    }

    fn buffer_open(id: EventId, target: Option<EntityRef>) -> Event {
        Event {
            id,
            name: "buffer/open".into(),
            target,
            payload: EventPayload::BufferOpen {
                path: "/tmp/x".into(),
            },
            provenance: Provenance::new(ActorIdentity::User, 0, None).unwrap(),
        }
    }

    #[test]
    fn rejects_event_id_zero() {
        let entity = EntityRef::new(7);
        let event = buffer_edit(EventId::ZERO, Some(entity), entity);
        let err = validate_event_envelope(&event).expect_err("ZERO must be rejected");
        assert!(err.contains("ZERO"), "error must name ZERO sentinel: {err}");
    }

    #[test]
    fn rejects_buffer_edit_with_target_payload_mismatch() {
        let event = buffer_edit(EventId::new(1), Some(EntityRef::new(1)), EntityRef::new(2));
        let err = validate_event_envelope(&event).expect_err("mismatch must be rejected");
        assert!(
            err.contains("BufferEdit envelope/payload mismatch"),
            "error must mention mismatch: {err}"
        );
        assert!(err.contains("target=1"), "error must name target: {err}");
        assert!(
            err.contains("payload.entity=2"),
            "error must name payload entity: {err}"
        );
    }

    #[test]
    fn rejects_buffer_edit_with_none_target() {
        let event = buffer_edit(EventId::new(1), None, EntityRef::new(2));
        let err = validate_event_envelope(&event).expect_err("None target must be rejected");
        assert!(
            err.contains("target=None"),
            "error must surface None: {err}"
        );
    }

    #[test]
    fn accepts_buffer_edit_with_consistent_target_and_entity() {
        let entity = EntityRef::new(42);
        let event = buffer_edit(EventId::new(1), Some(entity), entity);
        validate_event_envelope(&event).expect("consistent envelope must pass");
    }

    #[test]
    fn accepts_buffer_open_regardless_of_target() {
        // BufferOpen has no payload entity to compare; weaver-buffers
        // re-derives from the path. Any target should pass (or absent).
        validate_event_envelope(&buffer_open(EventId::new(1), Some(EntityRef::new(99))))
            .expect("BufferOpen with target must pass");
        validate_event_envelope(&buffer_open(EventId::new(1), None))
            .expect("BufferOpen without target must pass");
    }
}
