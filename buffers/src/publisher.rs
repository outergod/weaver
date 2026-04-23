//! Bus client: publishes `buffer/*` and `watcher/status` facts, owns
//! the per-buffer poll loop, manages the per-invocation
//! `ActorIdentity::Service` identity, and handles clean-shutdown
//! retraction and bus-EOF exit paths.
//!
//! Slice 003 builds this out across several commits. The current file
//! covers C10 (connect + handshake + reader_loop), C11 (service-level
//! and per-buffer bootstrap with fail-fast rollback on open failure),
//! C12 (poll loop with edge-triggered `buffer/dirty` and
//! `buffer/observable` transitions plus service-level degraded
//! aggregation), and C13 (clean-shutdown retract on SIGTERM / SIGINT,
//! bus-EOF classification). The CLI wrapper that invokes [`run`]
//! lands in C14.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::select;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use weaver_core::bus::client::{Client, ClientError};
use weaver_core::bus::codec::{CodecError, read_message, write_message};
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, LifecycleSignal};

use crate::model::{BufferState, ObserverError, watcher_instance_entity_ref};
use crate::observer;

/// Kebab-case service-id used in Hello / ActorIdentity / inspect
/// rendering, per `contracts/cli-surfaces.md` and Amendment 5.
const SERVICE_ID: &str = "weaver-buffers";

/// Thin wrapper around the write half of a bus connection — mirrors
/// slice-002's `BusWriter` so the publisher's `publish_*` helpers can
/// send without knowing whether the stream came from a `Client` or a
/// test harness.
struct BusWriter {
    writer: OwnedWriteHalf,
}

impl BusWriter {
    async fn send(&mut self, msg: &BusMessage) -> Result<(), ClientError> {
        write_message(&mut self.writer, msg).await?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum PublisherError {
    /// Bus not reachable: connect failed, handshake failed, or the
    /// reader task observed EOF (core gone). Exit code 2 per
    /// `contracts/cli-surfaces.md §Exit codes`.
    #[error("bus unavailable: {source}")]
    BusUnavailable {
        #[source]
        source: ClientError,
    },

    /// A `FactAssert` was rejected by the core's authority check
    /// (another service instance owns the claimed buffer entity).
    /// Exit code 3.
    #[error("authority conflict: {detail}")]
    AuthorityConflict { detail: String },

    /// A positional path could not be opened at startup (missing,
    /// directory, unreadable, etc.). Carries the categorised
    /// [`ObserverError`] so the CLI layer can render miette diagnostics
    /// with stable codes (WEAVER-BUF-001..003). Exit code 1.
    #[error("startup failure: {source}")]
    Observer {
        #[source]
        source: ObserverError,
    },

    /// Residual bus-client errors that don't map to the categories
    /// above. Exit code 10 (internal) per `research.md §9`; slice-002
    /// F31 follow-up reclassifies identity-drift / invalid-identity as
    /// fatal in a future soundness slice.
    #[error("bus client: {source}")]
    Client {
        #[source]
        source: ClientError,
    },
}

/// Run the publisher end-to-end.
///
/// Current scope:
///
/// 1. Connect + handshake (C10).
/// 2. Spawn reader task for server-sent Error frames (C10).
/// 3. Publish `watcher/status=started` on the instance entity (C11).
/// 4. For each path, in CLI order: `BufferState::open(path)` followed
///    by a 4-fact bootstrap with a per-buffer synthesised
///    [`EventId`] as `causal_parent`. Fail-fast on any open failure —
///    retract whatever partial bootstraps we published and return
///    [`PublisherError::Observer`] (C11).
/// 5. Publish `watcher/status=ready` (C11).
/// 6. Poll loop: per-tick per-buffer observation, edge-triggered
///    `buffer/dirty` / `buffer/observable`, service-level degraded
///    aggregation (C12).
/// 7. On SIGTERM / SIGINT: retract every `buffer/*` fact authored,
///    publish `watcher/status=unavailable` → `stopped`, close cleanly
///    with exit 0 (C13).
/// 8. On bus-EOF (core gone) or server-sent fatal errors: abort the
///    reader task, return the categorised [`PublisherError`] (C13).
pub async fn run(
    paths: Vec<PathBuf>,
    socket: PathBuf,
    poll_interval: Duration,
) -> Result<(), PublisherError> {
    let identity = ActorIdentity::service(SERVICE_ID, Uuid::new_v4())
        .expect("SERVICE_ID is constitutionally kebab-case");
    let instance_id = match &identity {
        ActorIdentity::Service { instance_id, .. } => *instance_id,
        _ => unreachable!("ActorIdentity::service returns a Service variant"),
    };
    let watcher_entity = watcher_instance_entity_ref(&instance_id);

    info!(
        socket = %socket.display(),
        poll_interval = ?poll_interval,
        instance = %instance_id,
        buffers = paths.len(),
        "weaver-buffers starting",
    );

    let client = Client::connect(&socket, SERVICE_ID)
        .await
        .map_err(|source| PublisherError::BusUnavailable { source })?;
    info!("connected to core; bus protocol handshake complete");

    let (reader, writer_half) = client.stream.into_split();
    let mut writer = BusWriter {
        writer: writer_half,
    };
    let (err_tx, mut err_rx) = mpsc::channel::<ServerSentError>(4);
    let reader_task = tokio::spawn(reader_loop(reader, err_tx));

    // Track every buffer/* fact we publish so the shutdown path (C13)
    // — and the fail-fast open-error branch below — can retract them
    // explicitly ahead of the bus disconnect. `watcher/status` is NOT
    // tracked here: it's a single-value family we overwrite via Stopped
    // rather than retract.
    let mut tracked: HashSet<FactKey> = HashSet::new();

    // T031: lifecycle started.
    publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Started,
    )
    .await?;

    // T030 + T032: per-buffer open + bootstrap in CLI order, fail-fast
    // with partial-retract on any open error.
    let mut states: Vec<BufferState> =
        match open_and_bootstrap_all(&mut writer, &identity, &paths, &mut tracked).await {
            Ok(states) => states,
            Err(e) => {
                // Retract any facts the partial-bootstrap published so
                // subscribers see retract-before-disconnect. Core's
                // release_connection would eventually cover it, but the
                // explicit order is a cleaner operator-observation contract
                // per T032.
                shutdown_retract(&mut writer, &identity, &mut tracked).await;
                reader_task.abort();
                return Err(e);
            }
        };

    // T031: lifecycle ready.
    publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Ready,
    )
    .await?;
    info!(
        facts_tracked = tracked.len(),
        "bootstrap complete; entering poll loop"
    );

    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();

    // T033 + T034 + T035 + T036: poll loop with edge-triggered
    // per-buffer `buffer/dirty` / `buffer/observable` transitions and
    // service-level `watcher/status=degraded` on all-unobservable
    // aggregate. Sequential iteration per buffer per research §10
    // (no scalability commitment in slice 003).
    let mut ticker = interval(poll_interval);
    ticker.tick().await; // burn the immediate first tick
    let mut was_degraded = false;

    let outcome: Result<(), PublisherError> = 'poll: loop {
        select! {
            _ = ticker.tick() => {}
            _ = wait_signal(&mut sigterm), if sigterm.is_some() => {
                info!("SIGTERM received; retracting facts and shutting down");
                clean_shutdown(&mut writer, watcher_entity, &identity, &mut tracked).await;
                break 'poll Ok(());
            }
            _ = wait_signal(&mut sigint), if sigint.is_some() => {
                info!("SIGINT received; retracting facts and shutting down");
                clean_shutdown(&mut writer, watcher_entity, &identity, &mut tracked).await;
                break 'poll Ok(());
            }
            maybe_err = err_rx.recv() => {
                // T038: no retract attempt on bus-EOF — the bus is
                // gone and any write would fail. Core's
                // release_connection covers cleanup server-side.
                break 'poll match maybe_err {
                    Some(err) => Err(translate_server_error(err)),
                    None => Err(bus_closed_error()),
                };
            }
        }

        // Per-tick event id: one synthesised EventId shared across
        // every transition this tick emits — per data-model.md,
        // retract/assert of `buffer/observable` and re-assert of
        // `buffer/dirty` correlate to the same poll tick.
        let poll_tick_id = EventId::new(now_ns());

        for state in &mut states {
            if let Err(e) =
                poll_tick_per_buffer(&mut writer, &identity, state, &mut tracked, poll_tick_id)
                    .await
            {
                break 'poll Err(e);
            }
        }

        // Service-level degraded aggregation (FR-016a).
        // `degraded` fires only when every currently-open buffer is
        // simultaneously unobservable; recovery (any buffer regains
        // observability) republishes `ready`. Edge-triggered.
        let all_unobservable = !states.is_empty() && states.iter().all(|s| !s.last_observable());
        match (all_unobservable, was_degraded) {
            (true, false) => {
                if let Err(e) = publish_watcher_status(
                    &mut writer,
                    watcher_entity,
                    &identity,
                    LifecycleSignal::Degraded,
                )
                .await
                {
                    break 'poll Err(e);
                }
                was_degraded = true;
            }
            (false, true) => {
                if let Err(e) = publish_watcher_status(
                    &mut writer,
                    watcher_entity,
                    &identity,
                    LifecycleSignal::Ready,
                )
                .await
                {
                    break 'poll Err(e);
                }
                was_degraded = false;
            }
            _ => {}
        }
    };

    reader_task.abort();
    outcome
}

/// Drive one poll tick for a single buffer: observe, then emit
/// edge-triggered `buffer/observable` + `buffer/dirty` facts only on
/// actual state changes. Updates `state.last_*` after successful
/// publish so the next tick's edge check is correct.
async fn poll_tick_per_buffer(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    state: &mut BufferState,
    tracked: &mut HashSet<FactKey>,
    poll_tick_id: EventId,
) -> Result<(), PublisherError> {
    let entity = state.entity();
    let causal = Some(poll_tick_id);
    match observer::observe_buffer(state) {
        Ok(obs) => {
            // Recovery: republish `observable=true` only when the
            // previous tick saw the buffer as unobservable.
            if !state.last_observable() {
                publish_fact(
                    writer,
                    FactKey::new(entity, "buffer/observable"),
                    FactValue::Bool(true),
                    identity,
                    causal,
                    tracked,
                )
                .await?;
                state.set_last_observable(true);
                debug!(
                    entity = %entity.as_u64(),
                    "buffer/observable=true (recovered)"
                );
            }
            // Dirty edge-trigger: republish only on flip.
            if obs.dirty != state.last_dirty() {
                publish_fact(
                    writer,
                    FactKey::new(entity, "buffer/dirty"),
                    FactValue::Bool(obs.dirty),
                    identity,
                    causal,
                    tracked,
                )
                .await?;
                state.set_last_dirty(obs.dirty);
                debug!(
                    entity = %entity.as_u64(),
                    dirty = obs.dirty,
                    "buffer/dirty transition published"
                );
            }
        }
        Err(e) => {
            // Unobservable edge-trigger: publish once on the
            // healthy→unobservable boundary; subsequent failed polls
            // remain silent until a successful observation recovers.
            if state.last_observable() {
                warn!(
                    path = %state.path().display(),
                    error = %e,
                    "buffer unobservable; flipping observable=false",
                );
                publish_fact(
                    writer,
                    FactKey::new(entity, "buffer/observable"),
                    FactValue::Bool(false),
                    identity,
                    causal,
                    tracked,
                )
                .await?;
                state.set_last_observable(false);
            } else {
                debug!(
                    path = %state.path().display(),
                    error = %e,
                    "buffer still unobservable; silent per edge-trigger rule",
                );
            }
        }
    }
    Ok(())
}

/// Iterate positional paths; open each buffer and publish its 4-fact
/// bootstrap with a per-buffer synthesised `EventId` as causal parent.
/// Returns the accumulated [`BufferState`]s on success so the poll loop
/// (C12) can consume them. On first open failure, surfaces a
/// [`PublisherError::Observer`]; the caller handles retraction of
/// whatever was already published.
async fn open_and_bootstrap_all(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    paths: &[PathBuf],
    tracked: &mut HashSet<FactKey>,
) -> Result<Vec<BufferState>, PublisherError> {
    let mut states = Vec::with_capacity(paths.len());
    for (idx, path) in paths.iter().enumerate() {
        let state = match BufferState::open(path.clone()) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    tracked_facts = tracked.len(),
                    "buffer open failed; aborting bootstrap",
                );
                return Err(PublisherError::Observer { source: e });
            }
        };
        // Per-buffer synthesised bootstrap-tick EventId. Deterministic:
        // the buffer's index in the (already de-duplicated) CLI order.
        // Research §8 + data-model §Bootstrap sequence step 3b.
        let bootstrap_tick = EventId::new(idx as u64);
        publish_buffer_bootstrap(writer, identity, &state, tracked, bootstrap_tick).await?;
        states.push(state);
    }
    Ok(states)
}

/// Publish a single buffer's 4-fact bootstrap set — path, byte-size,
/// dirty=false, observable=true — each carrying `bootstrap_tick` as
/// `causal_parent` so `why?` walks land on the buffer's own
/// synthesised boundary.
async fn publish_buffer_bootstrap(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    state: &BufferState,
    tracked: &mut HashSet<FactKey>,
    bootstrap_tick: EventId,
) -> Result<(), PublisherError> {
    let entity = state.entity();
    let causal = Some(bootstrap_tick);
    publish_fact(
        writer,
        FactKey::new(entity, "buffer/path"),
        FactValue::String(state.path().display().to_string()),
        identity,
        causal,
        tracked,
    )
    .await?;
    publish_fact(
        writer,
        FactKey::new(entity, "buffer/byte-size"),
        FactValue::U64(state.byte_size()),
        identity,
        causal,
        tracked,
    )
    .await?;
    publish_fact(
        writer,
        FactKey::new(entity, "buffer/dirty"),
        FactValue::Bool(false),
        identity,
        causal,
        tracked,
    )
    .await?;
    publish_fact(
        writer,
        FactKey::new(entity, "buffer/observable"),
        FactValue::Bool(true),
        identity,
        causal,
        tracked,
    )
    .await?;
    debug!(entity = %entity.as_u64(), "buffer bootstrap published");
    Ok(())
}

/// Reader task: drains server-sent `BusMessage`s from the read half,
/// filters `Error` frames into [`ServerSentError`] for the main loop,
/// and exits cleanly on EOF (dropping `err_tx`, which wakes the main
/// loop's `recv` arm with `None`).
async fn reader_loop(mut reader: OwnedReadHalf, err_tx: mpsc::Sender<ServerSentError>) {
    loop {
        match read_message(&mut reader).await {
            Ok(BusMessage::Error(msg)) => {
                let classified = classify_server_error(&msg.category, &msg.detail);
                let fatal = matches!(classified, ServerSentError::AuthorityConflict { .. });
                // Best-effort forward; a closed channel means the main
                // loop has already torn down and the send is moot.
                let _ = err_tx.send(classified).await;
                if fatal {
                    return;
                }
            }
            Ok(_) => {
                // Non-error server frames (SubscribeAck, lifecycle, facts
                // from peers) aren't actionable in the buffer service's
                // control flow yet; ignore.
            }
            Err(_) => {
                // EOF or codec error; drop err_tx by returning so the
                // main loop's recv arm surfaces None.
                return;
            }
        }
    }
}

/// Classify a server-sent `Error` frame by category string. Kept as a
/// standalone function so unit tests can exercise the mapping without
/// standing up a full bus connection.
fn classify_server_error(category: &str, detail: &str) -> ServerSentError {
    match category {
        "authority-conflict" => ServerSentError::AuthorityConflict {
            detail: detail.to_owned(),
        },
        "not-owner" => ServerSentError::NotOwner {
            detail: detail.to_owned(),
        },
        other => ServerSentError::Other {
            category: other.to_owned(),
            detail: detail.to_owned(),
        },
    }
}

/// Server-sent error classification. `AuthorityConflict` is fatal and
/// exits the service with code 3; `NotOwner` is treated as a soft
/// AuthorityConflict (prefixed detail) since hitting it in slice 003
/// implies the service is trying to retract a key it doesn't own,
/// which is a structural bug worth the same exit. `Other` is forwarded
/// for diagnostics and exits code 10 via `Client`.
#[derive(Debug)]
enum ServerSentError {
    AuthorityConflict { detail: String },
    NotOwner { detail: String },
    Other { category: String, detail: String },
}

fn translate_server_error(err: ServerSentError) -> PublisherError {
    match err {
        ServerSentError::AuthorityConflict { detail } => {
            PublisherError::AuthorityConflict { detail }
        }
        ServerSentError::NotOwner { detail } => PublisherError::AuthorityConflict {
            detail: format!("not-owner: {detail}"),
        },
        ServerSentError::Other { category, detail } => {
            warn!(%category, %detail, "server error forwarded as generic client error");
            PublisherError::Client {
                source: ClientError::Codec(CodecError::Io(std::io::Error::other(format!(
                    "server error {category}: {detail}"
                )))),
            }
        }
    }
}

fn bus_closed_error() -> PublisherError {
    PublisherError::BusUnavailable {
        source: ClientError::Codec(CodecError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "bus connection closed",
        ))),
    }
}

async fn wait_signal(sig: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(s) = sig.as_mut() {
        let _ = s.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn publish_watcher_status(
    writer: &mut BusWriter,
    watcher_entity: EntityRef,
    identity: &ActorIdentity,
    signal: LifecycleSignal,
) -> Result<(), PublisherError> {
    // Wire label: kebab-case label matching slice-002's encoding of
    // `LifecycleSignal` as a string FactValue on `watcher/status`.
    let label = match signal {
        LifecycleSignal::Started => "started",
        LifecycleSignal::Ready => "ready",
        LifecycleSignal::Degraded => "degraded",
        LifecycleSignal::Unavailable => "unavailable",
        LifecycleSignal::Restarting => "restarting",
        LifecycleSignal::Stopped => "stopped",
    };
    let prov = Provenance::new(identity.clone(), now_ns(), None)
        .expect("ActorIdentity is always well-formed");
    let fact = Fact {
        key: FactKey::new(watcher_entity, "watcher/status"),
        value: FactValue::String(label.into()),
        provenance: prov,
    };
    writer
        .send(&BusMessage::FactAssert(fact))
        .await
        .map_err(|source| PublisherError::Client { source })?;
    // Not tracked for shutdown-retract: we overwrite to Stopped rather
    // than retract on clean exit.
    Ok(())
}

async fn publish_fact(
    writer: &mut BusWriter,
    key: FactKey,
    value: FactValue,
    identity: &ActorIdentity,
    causal_parent: Option<EventId>,
    tracked: &mut HashSet<FactKey>,
) -> Result<(), PublisherError> {
    let prov = Provenance::new(identity.clone(), now_ns(), causal_parent)
        .expect("ActorIdentity is always well-formed");
    let fact = Fact {
        key: key.clone(),
        value,
        provenance: prov,
    };
    writer
        .send(&BusMessage::FactAssert(fact))
        .await
        .map_err(|source| PublisherError::Client { source })?;
    tracked.insert(key);
    Ok(())
}

/// Retract every fact in `tracked` on a best-effort basis. Broken-pipe
/// writes are ignored — we're shutting down and the core's
/// release_connection covers anything we miss.
async fn shutdown_retract(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    tracked: &mut HashSet<FactKey>,
) {
    let keys: Vec<FactKey> = tracked.drain().collect();
    for key in keys {
        let prov = match Provenance::new(identity.clone(), now_ns(), None) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let _ = writer
            .send(&BusMessage::FactRetract {
                key,
                provenance: prov,
            })
            .await;
    }
}

/// Clean shutdown sequence driven by SIGTERM / SIGINT: retract every
/// tracked `buffer/*` fact, then overwrite `watcher/status` with
/// `unavailable` → `stopped`. Every write is best-effort — if the bus
/// dies mid-shutdown the core's `release_connection` covers the gap.
/// Kept out of the fail-fast open-error path per
/// `contracts/bus-messages.md §Failure modes`: startup failures
/// announce via stderr + retract only, no bus-level lifecycle.
async fn clean_shutdown(
    writer: &mut BusWriter,
    watcher_entity: EntityRef,
    identity: &ActorIdentity,
    tracked: &mut HashSet<FactKey>,
) {
    shutdown_retract(writer, identity, tracked).await;
    let _ = publish_watcher_status(
        writer,
        watcher_entity,
        identity,
        LifecycleSignal::Unavailable,
    )
    .await;
    let _ =
        publish_watcher_status(writer, watcher_entity, identity, LifecycleSignal::Stopped).await;
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_server_error_maps_authority_conflict() {
        let c = classify_server_error("authority-conflict", "buffer/path already claimed");
        assert!(matches!(c, ServerSentError::AuthorityConflict { .. }));
    }

    #[test]
    fn classify_server_error_maps_not_owner() {
        let c = classify_server_error("not-owner", "retract for key not owned");
        assert!(matches!(c, ServerSentError::NotOwner { .. }));
    }

    #[test]
    fn classify_server_error_maps_other_categories() {
        for cat in ["identity-drift", "invalid-identity", "decode", "unknown-x"] {
            let c = classify_server_error(cat, "irrelevant");
            match c {
                ServerSentError::Other { category, .. } => assert_eq!(category, cat),
                other => panic!("expected Other for {cat}, got {other:?}"),
            }
        }
    }

    #[test]
    fn translate_server_error_preserves_authority_conflict_detail() {
        let err = translate_server_error(ServerSentError::AuthorityConflict {
            detail: "d1".into(),
        });
        match err {
            PublisherError::AuthorityConflict { detail } => assert_eq!(detail, "d1"),
            other => panic!("expected AuthorityConflict, got {other:?}"),
        }
    }

    #[test]
    fn translate_server_error_coerces_not_owner_to_authority_conflict() {
        let err = translate_server_error(ServerSentError::NotOwner {
            detail: "key/x".into(),
        });
        match err {
            PublisherError::AuthorityConflict { detail } => {
                assert!(
                    detail.starts_with("not-owner:"),
                    "NotOwner must carry its prefix: {detail}"
                );
            }
            other => panic!("expected AuthorityConflict, got {other:?}"),
        }
    }

    #[test]
    fn translate_server_error_funnels_other_into_client_error() {
        let err = translate_server_error(ServerSentError::Other {
            category: "identity-drift".into(),
            detail: "why".into(),
        });
        assert!(matches!(err, PublisherError::Client { .. }));
    }

    #[test]
    fn bus_closed_error_surfaces_as_bus_unavailable() {
        let err = bus_closed_error();
        assert!(matches!(err, PublisherError::BusUnavailable { .. }));
    }

    #[test]
    fn publisher_error_observer_preserves_source() {
        let src = ObserverError::StartupFailure {
            path: std::path::PathBuf::from("/nope"),
            reason: "missing".into(),
        };
        let err = PublisherError::Observer { source: src };
        assert!(format!("{err}").starts_with("startup failure:"));
        match err {
            PublisherError::Observer {
                source: ObserverError::StartupFailure { path, .. },
            } => {
                assert_eq!(path, std::path::PathBuf::from("/nope"));
            }
            other => panic!("expected Observer(StartupFailure), got {other:?}"),
        }
    }
}
