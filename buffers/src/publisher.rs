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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
use weaver_core::types::edit::TextEdit;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, LifecycleSignal};

use crate::model::{
    ApplyError, BufferState, ObserverError, buffer_bootstrap_facts, buffer_entity_ref,
    watcher_instance_entity_ref,
};
use crate::observer;

/// Kebab-case service-id used in Hello / ActorIdentity / inspect
/// rendering, per `contracts/cli-surfaces.md` and Amendment 5.
const SERVICE_ID: &str = "weaver-buffers";

/// Window to drain pending `authority-conflict` / other bus errors
/// between bootstrap-fact emission and the `watcher/status=ready` +
/// `buffer/open` event burst. Conflicts are reported asynchronously
/// by the core via `err_rx`; without this gate, a doomed instance
/// would publish `ready` (and `buffer/open` events) before its
/// bootstrap `FactAssert` was actually accepted.
const BOOTSTRAP_GRACE: Duration = Duration::from_millis(250);

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

/// Per-instance buffer registry: holds owned [`BufferState`]s and the
/// `buffer/version` counters for each owned entity.
///
/// The `buffers` map is the single source of truth for ownership
/// (entity ownership lookup is `buffers.contains_key(&e)`) and for the
/// in-memory content the poll loop and the slice-004 edit-dispatch
/// arm operate on. The `versions` map tracks per-buffer
/// `buffer/version`: initialised to `0` at bootstrap (matches the
/// slice-003 PR #10 forward-compat bootstrap fact); bumped by each
/// accepted `EventPayload::BufferEdit` in slice 004; never
/// decremented.
///
/// Drives the FR-011a idempotence invariant for `BufferOpen`: a
/// repeat dispatch whose derived entity is already in `buffers`
/// short-circuits to a no-op — no re-read, no fact re-publication,
/// no trace emission. Slice-003's CLI bootstrap deduplicates paths
/// at parse time (T055) so the already-owned branch is unreachable
/// under slice-003 argv; slice 004+ external producers that emit
/// `BufferOpen` over the wire will exercise the branch for real.
#[derive(Default)]
pub(crate) struct BufferRegistry {
    pub(crate) buffers: HashMap<EntityRef, BufferState>,
    pub(crate) versions: HashMap<EntityRef, u64>,
}

impl BufferRegistry {
    pub(crate) fn is_owned(&self, entity: EntityRef) -> bool {
        self.buffers.contains_key(&entity)
    }

    /// Insert a freshly-opened buffer's state and initialise its
    /// `buffer/version` counter to `0`. Caller MUST have confirmed
    /// `is_owned(state.entity()) == false` (the FR-011a check happens
    /// at [`dispatch_buffer_open`]'s decision point, not here).
    pub(crate) fn insert(&mut self, state: BufferState) {
        let entity = state.entity();
        self.versions.insert(entity, 0);
        self.buffers.insert(entity, state);
    }
}

/// Outcome of dispatching a `BufferOpen` for a given canonical
/// path. The caller decides what to do with each variant; the
/// idempotence invariant lives in the handler, not the caller.
#[derive(Debug)]
pub(crate) enum BufferOpenOutcome {
    /// First sighting — caller MUST publish the 4-fact bootstrap
    /// and [`BufferRegistry::insert`] the returned state so
    /// subsequent dispatches short-circuit.
    Fresh(BufferState),
    /// Entity already owned. Caller MUST NOT publish or retract
    /// anything — FR-011a.
    AlreadyOwned,
}

/// Handler for a single `BufferOpen` event: decides fresh-vs-
/// already-owned and, when fresh, opens the file. Kept as a
/// pure-ish function (no bus writes, no tracing beyond the
/// `AlreadyOwned` debug line) so the unit test can exercise
/// idempotence without a mock writer.
pub(crate) fn dispatch_buffer_open(
    registry: &BufferRegistry,
    path: &Path,
) -> Result<BufferOpenOutcome, ObserverError> {
    let entity = buffer_entity_ref(path);
    if registry.is_owned(entity) {
        debug!(
            entity = %entity.as_u64(),
            path = %path.display(),
            "BufferOpen for already-owned entity; no-op per FR-011a",
        );
        return Ok(BufferOpenOutcome::AlreadyOwned);
    }
    let state = BufferState::open(path.to_path_buf())?;
    Ok(BufferOpenOutcome::Fresh(state))
}

/// Outcome of dispatching a single `BufferEdit` event for a given
/// `(entity, emitted_version)` tuple. Mirrors slice-003's
/// [`BufferOpenOutcome`] shape: pure-ish dispatch returns one variant
/// per receipt; the caller (the reader-loop arm in [`reader_loop`]
/// once T010 wires it) decides what to publish.
///
/// All non-`Applied` variants are silent drops on the wire per
/// FR-018; the publisher emits a categorised `tracing::debug!` line
/// keyed off the variant so post-mortem trace inspection can
/// reconstruct *why* an edit was dropped without any subscriber
/// observing the rejection.
//
// `dead_code` is silenced until T010 wires this into `reader_loop`;
// the enum is fully exercised by the unit tests below.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum BufferEditOutcome {
    /// Validated batch was applied. The post-apply state lives in
    /// `registry.buffers[entity]`; the variant carries the metadata
    /// the reader-loop arm needs to publish `buffer/byte-size` and
    /// `buffer/version` directly, plus the post-apply
    /// `memory_digest` for the dirty calculation (which still
    /// requires re-reading the file from disk).
    Applied {
        entity: EntityRef,
        new_version: u64,
        new_byte_size: u64,
        new_memory_digest: [u8; 32],
    },
    /// This service does not own the target entity (no `BufferOpen`
    /// has been processed for it on this instance). No publish.
    NotOwned,
    /// Emitted version is older than current — almost certainly a
    /// concurrent-edit conflict where two emitters saw the same
    /// version and one landed first. Drop per FR-005 step 2.
    StaleVersion { current: u64, emitted: u64 },
    /// Emitted version is newer than current — defensive drop for an
    /// emitter that saw a version this service did not issue.
    FutureVersion { current: u64, emitted: u64 },
    /// Per-edit or batch-overlap validation rejected the batch. No
    /// state mutation per [`BufferState::apply_edits`]'s atomicity
    /// guarantee; no publish.
    ValidationFailure(ApplyError),
}

/// Handler for a single `BufferEdit` event: gates ownership + version,
/// applies the batch atomically (via [`BufferState::apply_edits`]),
/// and bumps the per-buffer version on accept. Pure-ish (no bus
/// writes, no tracing) so unit tests exercise the full outcome matrix
/// without a mock writer; FR-018 categorisation lives in the
/// reader-loop arm that consumes the outcome.
///
/// Ordering is significant:
///
/// 1. Ownership gate — non-owned entities short-circuit first; we
///    must not even peek at the version map for entities other
///    services own.
/// 2. Version gate — stale and future drops happen before validation
///    so a malformed batch under a concurrent-edit conflict surfaces
///    as a version drop, not a validation failure (the operator's
///    diagnostic priority is the conflict, not the malformed edit).
/// 3. Validation + apply — `BufferState::apply_edits` validates the
///    full batch before mutating anything; on failure the buffer is
///    untouched.
/// 4. Version bump — only on accepted apply.
//
// `dead_code` is silenced until T010 wires this into `reader_loop`;
// the function is fully exercised by the unit tests below.
#[allow(dead_code)]
pub(crate) fn dispatch_buffer_edit(
    registry: &mut BufferRegistry,
    entity: EntityRef,
    version: u64,
    edits: &[TextEdit],
) -> BufferEditOutcome {
    if !registry.is_owned(entity) {
        return BufferEditOutcome::NotOwned;
    }
    let current = registry.versions.get(&entity).copied().unwrap_or(0);
    if version < current {
        return BufferEditOutcome::StaleVersion {
            current,
            emitted: version,
        };
    }
    if version > current {
        return BufferEditOutcome::FutureVersion {
            current,
            emitted: version,
        };
    }
    // version == current — eligible for application.
    let state = registry
        .buffers
        .get_mut(&entity)
        .expect("is_owned(entity) implies buffers contains the key");
    if let Err(err) = state.apply_edits(edits) {
        return BufferEditOutcome::ValidationFailure(err);
    }
    let new_byte_size = state.byte_size();
    let new_memory_digest = *state.memory_digest();
    let new_version = current + 1;
    registry.versions.insert(entity, new_version);
    BufferEditOutcome::Applied {
        entity,
        new_version,
        new_byte_size,
        new_memory_digest,
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
    let mut registry = BufferRegistry::default();
    let bootstrap_anchors: Vec<(EntityRef, EventId)> =
        match open_and_bootstrap_all(&mut writer, &identity, &paths, &mut tracked, &mut registry)
            .await
        {
            Ok(anchors) => anchors,
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

    // Bootstrap-write sides returned Ok (wire writes succeeded), but
    // authority-conflict rejections come back asynchronously on
    // `err_rx`. Drain for a grace window before the `ready` + per-
    // buffer `buffer/open` event burst so:
    //
    //   - `watcher/status=ready` means "bootstrap facts accepted",
    //     not merely "wire writes returned Ok" — matches the
    //     contract subscribers rely on.
    //   - `BusMessage::Event(EventPayload::BufferOpen { .. })` only
    //     fires for buffers this instance actually owns. Events are
    //     lossy-class (no retract), so emitting them pre-drain would
    //     produce false-positive open signals for doomed instances
    //     that subscribers could never unsee.
    if let Some(async_err) = wait_for_bootstrap_error(&mut err_rx, BOOTSTRAP_GRACE).await {
        shutdown_retract(&mut writer, &identity, &mut tracked).await;
        reader_task.abort();
        return Err(async_err);
    }

    // Drain cleared: the core accepted every bootstrap `FactAssert`.
    // Now emit the `buffer/open` events that anchor each buffer's
    // causal_parent chain, in the same CLI order the bootstrap facts
    // used. Each anchor pairs the entity with the bootstrap-tick
    // `EventId` that was already used as the bootstrap `causal_parent`,
    // so `weaver inspect --why` walkbacks land on the matching event.
    for (entity, bootstrap_id) in &bootstrap_anchors {
        let state = registry
            .buffers
            .get(entity)
            .expect("bootstrap_anchors mirrors registry.buffers keys");
        publish_buffer_open_event(&mut writer, &identity, state, *bootstrap_id).await?;
    }

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

        for state in registry.buffers.values_mut() {
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
        let all_unobservable =
            !registry.buffers.is_empty() && registry.buffers.values().all(|s| !s.last_observable());
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
///
/// Routes every path through [`dispatch_buffer_open`] so the CLI hot
/// path and any future (slice 004+) wire-driven `BufferOpen` handler
/// share the same idempotence invariant. T055's CLI-side dedup makes
/// the `AlreadyOwned` branch unreachable under slice-003 argv, but
/// threading the registry here keeps it authoritative if a wire-side
/// handler ever joins the same publisher instance.
async fn open_and_bootstrap_all(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    paths: &[PathBuf],
    tracked: &mut HashSet<FactKey>,
    registry: &mut BufferRegistry,
) -> Result<Vec<(EntityRef, EventId)>, PublisherError> {
    let mut anchors = Vec::with_capacity(paths.len());
    for (idx, path) in paths.iter().enumerate() {
        let outcome = match dispatch_buffer_open(registry, path) {
            Ok(o) => o,
            Err(source) => {
                warn!(
                    path = %path.display(),
                    error = %source,
                    tracked_facts = tracked.len(),
                    "buffer open failed; aborting bootstrap",
                );
                return Err(PublisherError::Observer { source });
            }
        };
        let state = match outcome {
            BufferOpenOutcome::Fresh(s) => s,
            BufferOpenOutcome::AlreadyOwned => {
                // Unreachable under slice-003 argv (T055 dedups
                // upstream). If a future caller triggers this, FR-011a
                // demands a silent skip — no re-bootstrap, no retract.
                continue;
            }
        };
        // Per-buffer synthesised bootstrap-tick EventId. Deterministic:
        // the buffer's index in the (already de-duplicated) CLI order.
        // Research §8 + data-model §Bootstrap sequence step 3b.
        //
        // Bootstrap facts carry this id as `causal_parent`; the
        // matching `BusMessage::Event(EventPayload::BufferOpen { .. })`
        // is emitted by the caller AFTER `wait_for_bootstrap_error`
        // confirms no async authority-conflict on any fact in this
        // batch. Emitting the event pre-drain would produce a
        // false-positive `buffer/open` signal for doomed instances
        // (events are lossy-class, no retract), so the caller owns
        // the "event emission only once ownership is confirmed"
        // ordering.
        let bootstrap_tick = EventId::new(idx as u64);
        let entity = state.entity();
        publish_buffer_bootstrap(writer, identity, &state, tracked, bootstrap_tick).await?;
        registry.insert(state);
        anchors.push((entity, bootstrap_tick));
    }
    Ok(anchors)
}

/// Publish the `BufferOpen` event that anchors a buffer's bootstrap
/// fact set. Carries the same [`EventId`] the bootstrap facts will
/// use as `causal_parent`, so `weaver inspect --why` walkback from
/// any of those facts lands on this event.
///
/// `target` is the buffer entity the open claims; `payload` carries
/// the canonical path (rendered via `Display`). Provenance is the
/// service identity at the current wall-clock, with no causal
/// parent — the open is the origin of the buffer's lifecycle within
/// this invocation.
///
/// Events are lossy-class (no authority check, not tracked for
/// retraction); shutdown only retracts the bootstrap facts.
async fn publish_buffer_open_event(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    state: &BufferState,
    bootstrap_tick: EventId,
) -> Result<(), PublisherError> {
    let prov = Provenance::new(identity.clone(), now_ns(), None)
        .expect("ActorIdentity is always well-formed");
    let event = Event {
        id: bootstrap_tick,
        name: "buffer/open".into(),
        target: Some(state.entity()),
        payload: EventPayload::BufferOpen {
            path: state.path().display().to_string(),
        },
        provenance: prov,
    };
    writer
        .send(&BusMessage::Event(event))
        .await
        .map_err(classify_write_error)?;
    Ok(())
}

/// Publish a single buffer's 4-fact bootstrap set — path, byte-size,
/// dirty=false, observable=true — each carrying `bootstrap_tick` as
/// `causal_parent` so `why?` walks land on the buffer's own
/// synthesised boundary.
///
/// The `(attribute, FactValue)` tuples come from
/// [`buffer_bootstrap_facts`]; keeping the map in one place lets the
/// SC-306 component-discipline proptest (T062) exercise the exact
/// shape the wire sees.
async fn publish_buffer_bootstrap(
    writer: &mut BusWriter,
    identity: &ActorIdentity,
    state: &BufferState,
    tracked: &mut HashSet<FactKey>,
    bootstrap_tick: EventId,
) -> Result<(), PublisherError> {
    let entity = state.entity();
    let causal = Some(bootstrap_tick);
    for (attribute, value) in buffer_bootstrap_facts(state) {
        publish_fact(
            writer,
            FactKey::new(entity, attribute),
            value,
            identity,
            causal,
            tracked,
        )
        .await?;
    }
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

/// Poll `err_rx` for up to `window` to catch any asynchronous bus
/// error queued by `reader_loop` since the bootstrap writes started.
/// Used to gate `watcher/status=ready` and the per-buffer
/// `buffer/open` events on confirmed bootstrap-fact acceptance.
///
/// Returns `Some(PublisherError)` if an error or channel-close
/// surfaces within the window; `None` if no error arrived (the
/// clean path).
async fn wait_for_bootstrap_error(
    err_rx: &mut mpsc::Receiver<ServerSentError>,
    window: Duration,
) -> Option<PublisherError> {
    match tokio::time::timeout(window, err_rx.recv()).await {
        Err(_elapsed) => None,
        Ok(Some(err)) => Some(translate_server_error(err)),
        Ok(None) => Some(bus_closed_error()),
    }
}

/// Classify a write-side [`ClientError`] from `writer.send(...)`.
///
/// The reader loop's EOF path maps bus death to
/// [`PublisherError::BusUnavailable`] (exit code 2). Without this
/// helper, a writer that loses the peer between the poll-loop
/// `select!` arm and the send would surface `BrokenPipe` /
/// `ConnectionReset` / `UnexpectedEof` and get funnelled into
/// [`PublisherError::Client`] (exit code 10 — internal), so the
/// same disconnect produces a different exit code depending on
/// which side of the socket notices first.
///
/// This helper recovers the symmetry: any transport-level failure
/// maps to `BusUnavailable`; only encoding / frame-size / handshake-
/// protocol errors (which indicate a programmer bug, not a dead
/// peer) remain `Client`.
fn classify_write_error(source: ClientError) -> PublisherError {
    if let ClientError::Codec(CodecError::Io(ref io_err)) = source {
        match io_err.kind() {
            std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected => {
                return PublisherError::BusUnavailable { source };
            }
            _ => {}
        }
    }
    PublisherError::Client { source }
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
        .map_err(classify_write_error)?;
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
        .map_err(classify_write_error)?;
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
    fn classify_write_error_maps_transport_kinds_to_bus_unavailable() {
        // Every io::ErrorKind that indicates the peer is gone must
        // surface as BusUnavailable so the exit code matches the
        // reader-loop EOF path (exit 2), not Client (exit 10).
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::NotConnected,
        ] {
            let src = ClientError::Codec(CodecError::Io(std::io::Error::new(kind, "peer gone")));
            let err = classify_write_error(src);
            assert!(
                matches!(err, PublisherError::BusUnavailable { .. }),
                "io::ErrorKind::{kind:?} must classify as BusUnavailable; got {err:?}"
            );
        }
    }

    #[test]
    fn classify_write_error_preserves_client_for_encoding_errors() {
        // Encoding / frame-size / decode errors are programmer bugs,
        // not dead peers; they must NOT be laundered into
        // BusUnavailable.
        let src = ClientError::Codec(CodecError::Encode("bad encoding".into()));
        let err = classify_write_error(src);
        assert!(
            matches!(err, PublisherError::Client { .. }),
            "Encode error must classify as Client; got {err:?}"
        );

        let src = ClientError::Codec(CodecError::FrameTooLarge {
            size: 1_000_000,
            max: 65536,
        });
        let err = classify_write_error(src);
        assert!(matches!(err, PublisherError::Client { .. }));
    }

    #[test]
    fn classify_write_error_preserves_client_for_unrelated_io_kinds() {
        // An io::Error that isn't one of the transport-death kinds
        // stays Client. NotFound / PermissionDenied on a write would
        // be bizarre but not a bus disconnect.
        let src = ClientError::Codec(CodecError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "eacces",
        )));
        let err = classify_write_error(src);
        assert!(
            matches!(err, PublisherError::Client { .. }),
            "unexpected io kind must classify as Client; got {err:?}"
        );
    }

    #[test]
    fn dispatch_buffer_open_is_noop_for_already_owned_entity() {
        use std::io::Write;
        // FR-011a: a second `BufferOpen` for a canonical path whose
        // derived entity is already owned must short-circuit to a
        // no-op. The handler surfaces this by returning the
        // `AlreadyOwned` variant — carrying no `BufferState`, which
        // means a disciplined caller has nothing to publish or
        // retract. That's the "fires no FactAssert / FactRetract"
        // contract T057 asserts, demonstrated without a mock writer.
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(b"slice-003 idempotence fixture\n")
            .expect("write");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");

        let mut registry = BufferRegistry::default();

        // First dispatch: fresh. Caller (the bootstrap loop) would
        // publish the 4-fact bootstrap and insert the state into the
        // registry — which simultaneously sets the version counter to
        // 0 and marks the entity as owned (HashMap keyset is the
        // ownership marker post-T008).
        let first =
            dispatch_buffer_open(&registry, &canonical).expect("first dispatch must succeed");
        let state = match first {
            BufferOpenOutcome::Fresh(s) => s,
            BufferOpenOutcome::AlreadyOwned => panic!("first dispatch must be Fresh"),
        };
        assert_eq!(state.path(), canonical.as_path());
        let entity = state.entity();
        registry.insert(state);
        assert!(registry.is_owned(entity));
        assert_eq!(registry.versions.get(&entity).copied(), Some(0));

        // Second dispatch on the same canonical path: registry hit,
        // so the handler returns AlreadyOwned and performs no file
        // I/O beyond the registry lookup.
        let second =
            dispatch_buffer_open(&registry, &canonical).expect("second dispatch must succeed");
        match second {
            BufferOpenOutcome::AlreadyOwned => {}
            BufferOpenOutcome::Fresh(_) => {
                panic!("second dispatch on already-owned entity must be AlreadyOwned (FR-011a)")
            }
        }
    }

    #[test]
    fn dispatch_buffer_open_passes_observer_errors_through() {
        // A missing path must surface as `ObserverError::StartupFailure`
        // rather than being masked by the registry short-circuit.
        let registry = BufferRegistry::default();
        let missing = std::path::PathBuf::from("/definitely/not/a/real/path/weaver-dispatch-test");
        let err =
            dispatch_buffer_open(&registry, &missing).expect_err("missing path must fail the open");
        assert!(matches!(err, ObserverError::StartupFailure { .. }));
    }

    #[test]
    fn publisher_error_observer_preserves_source() {
        let src = ObserverError::StartupFailure {
            path: std::path::PathBuf::from("/nope"),
            reason: "missing".into(),
            kind: crate::model::StartupKind::NotOpenable,
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

    // ───────────────────────────────────────────────────────────────────
    // Slice-004: dispatch_buffer_edit + BufferEditOutcome (T009).
    // ───────────────────────────────────────────────────────────────────

    use weaver_core::types::edit::{Position, Range, TextEdit};

    /// Build a single-buffer registry and return it together with the
    /// derived entity and the backing tempfile (caller binds the
    /// tempfile to keep the disk path alive for the test scope, even
    /// though dispatch_buffer_edit operates on in-memory content only).
    fn registry_with_buffer(
        content: &[u8],
    ) -> (BufferRegistry, EntityRef, tempfile::NamedTempFile) {
        use std::io::Write;
        let mut tf = tempfile::NamedTempFile::new().expect("tempfile");
        tf.write_all(content).expect("write");
        let canonical = std::fs::canonicalize(tf.path()).expect("canonicalize");
        let state = BufferState::open(canonical).expect("open");
        let entity = state.entity();
        let mut reg = BufferRegistry::default();
        reg.insert(state);
        (reg, entity, tf)
    }

    fn pure_insert(line: u32, character: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position { line, character },
                end: Position { line, character },
            },
            new_text: text.into(),
        }
    }

    #[test]
    fn dispatch_buffer_edit_returns_not_owned_for_unknown_entity() {
        let mut reg = BufferRegistry::default();
        let entity = EntityRef::new(0xDEAD_BEEF);
        let outcome = dispatch_buffer_edit(&mut reg, entity, 0, &[]);
        assert!(matches!(outcome, BufferEditOutcome::NotOwned));
    }

    #[test]
    fn dispatch_buffer_edit_returns_stale_version_when_emitted_below_current() {
        let (mut reg, entity, _tf) = registry_with_buffer(b"hello");
        // Bump current to 5 to set up the stale scenario.
        reg.versions.insert(entity, 5);
        let outcome = dispatch_buffer_edit(&mut reg, entity, 3, &[]);
        match outcome {
            BufferEditOutcome::StaleVersion { current, emitted } => {
                assert_eq!(current, 5);
                assert_eq!(emitted, 3);
            }
            other => panic!("expected StaleVersion, got {other:?}"),
        }
        // No mutation: version stays at 5.
        assert_eq!(reg.versions.get(&entity).copied(), Some(5));
    }

    #[test]
    fn dispatch_buffer_edit_returns_future_version_when_emitted_above_current() {
        let (mut reg, entity, _tf) = registry_with_buffer(b"hello");
        reg.versions.insert(entity, 5);
        let outcome = dispatch_buffer_edit(&mut reg, entity, 7, &[]);
        match outcome {
            BufferEditOutcome::FutureVersion { current, emitted } => {
                assert_eq!(current, 5);
                assert_eq!(emitted, 7);
            }
            other => panic!("expected FutureVersion, got {other:?}"),
        }
        assert_eq!(reg.versions.get(&entity).copied(), Some(5));
    }

    #[test]
    fn dispatch_buffer_edit_returns_validation_failure_for_malformed_batch() {
        // Buffer "hi" — line 0 length is 2; character 99 is OOB.
        let (mut reg, entity, _tf) = registry_with_buffer(b"hi");
        let bad = pure_insert(0, 99, "boom");
        let outcome = dispatch_buffer_edit(&mut reg, entity, 0, std::slice::from_ref(&bad));
        match outcome {
            BufferEditOutcome::ValidationFailure(err) => {
                assert_eq!(err.reason(), "validation-failure-out-of-bounds");
                assert_eq!(err.edit_index(), Some(0));
            }
            other => panic!("expected ValidationFailure, got {other:?}"),
        }
        // Atomicity: version + content unchanged.
        assert_eq!(reg.versions.get(&entity).copied(), Some(0));
        assert_eq!(reg.buffers.get(&entity).unwrap().content(), b"hi");
    }

    #[test]
    fn dispatch_buffer_edit_returns_applied_for_valid_batch() {
        use sha2::{Digest, Sha256};
        let (mut reg, entity, _tf) = registry_with_buffer(b"world");
        let edit = pure_insert(0, 0, "hello ");
        let outcome = dispatch_buffer_edit(&mut reg, entity, 0, std::slice::from_ref(&edit));
        match outcome {
            BufferEditOutcome::Applied {
                entity: e,
                new_version,
                new_byte_size,
                new_memory_digest,
            } => {
                assert_eq!(e, entity);
                assert_eq!(new_version, 1);
                assert_eq!(new_byte_size, b"hello world".len() as u64);
                let expected: [u8; 32] = Sha256::digest(b"hello world").into();
                assert_eq!(new_memory_digest, expected);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        // Registry post-state matches the outcome's snapshot.
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));
        let state = reg.buffers.get(&entity).unwrap();
        assert_eq!(state.content(), b"hello world");
        assert_eq!(state.byte_size(), b"hello world".len() as u64);
    }

    #[test]
    fn dispatch_buffer_edit_increments_version_only_on_accept() {
        let (mut reg, entity, _tf) = registry_with_buffer(b"x");
        let valid = pure_insert(0, 0, "a");

        // Accept: bumps to 1.
        let _ = dispatch_buffer_edit(&mut reg, entity, 0, std::slice::from_ref(&valid));
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));

        // Stale (emit=0, current=1): no bump.
        let _ = dispatch_buffer_edit(&mut reg, entity, 0, std::slice::from_ref(&valid));
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));

        // Future (emit=99, current=1): no bump.
        let _ = dispatch_buffer_edit(&mut reg, entity, 99, std::slice::from_ref(&valid));
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));

        // Validation failure at the correct version: no bump.
        let bad = pure_insert(0, 99, "boom");
        let _ = dispatch_buffer_edit(&mut reg, entity, 1, std::slice::from_ref(&bad));
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));
    }

    #[test]
    fn dispatch_buffer_edit_empty_batch_at_correct_version_bumps_version() {
        // Per FR-008 + data-model: empty batch is structurally an
        // identity on content, but it IS still an accepted edit at
        // the wire level; the version bumps by 1. (Subscribers see
        // an empty re-emit burst — byte-size unchanged but version
        // advances.) This pins the boundary between "dropped at
        // wire" and "applied as identity".
        let (mut reg, entity, _tf) = registry_with_buffer(b"hello");
        let outcome = dispatch_buffer_edit(&mut reg, entity, 0, &[]);
        match outcome {
            BufferEditOutcome::Applied { new_version, .. } => assert_eq!(new_version, 1),
            other => panic!("expected Applied (empty batch), got {other:?}"),
        }
        assert_eq!(reg.versions.get(&entity).copied(), Some(1));
        assert_eq!(reg.buffers.get(&entity).unwrap().content(), b"hello");
    }
}
