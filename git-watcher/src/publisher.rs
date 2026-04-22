//! Bus-client publisher for `weaver-git-watcher`. Maintains `repo/*`
//! fact-family authority for one repository under a structured
//! `ActorIdentity::Service` (Clarification Q1) with a random UUID v4
//! per invocation (Clarification Q3).
//!
//! See `specs/002-git-watcher-actor/` — spec.md, data-model.md,
//! contracts/bus-messages.md, contracts/cli-surfaces.md — for the
//! binding shape.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, LifecycleSignal};

use crate::model::WorkingCopyState;
use crate::observer::{Observation, RepoObserver};

/// Thin wrapper around the write half of a bus connection. Exists so
/// the publisher's `publish_*` helpers can stay client-agnostic: they
/// call `BusWriter::send(&BusMessage)` without knowing whether the
/// stream was split off a `Client` or not. After slice-002's F3 fix
/// the watcher splits its stream post-handshake so a reader task can
/// surface server-sent `Error` frames (authority-conflict, not-owner)
/// to the main loop while writes continue concurrently.
pub struct BusWriter {
    writer: OwnedWriteHalf,
}

impl BusWriter {
    pub async fn send(&mut self, msg: &BusMessage) -> Result<(), ClientError> {
        write_message(&mut self.writer, msg).await?;
        Ok(())
    }
}

/// Default bus socket path, matching the core's `cli::config::Config`
/// default. Overridable via `--socket` on the watcher CLI.
fn default_socket() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return Path::new(&runtime_dir).join("weaver.sock");
    }
    PathBuf::from("/tmp/weaver.sock")
}

#[derive(Debug, Error)]
pub enum PublisherError {
    #[error("bus unavailable: {source}")]
    BusUnavailable {
        #[source]
        source: ClientError,
    },

    #[error("authority conflict: {detail}")]
    AuthorityConflict { detail: String },

    #[error("observation failed: {source}")]
    Observer {
        #[source]
        source: crate::model::ObserverError,
    },

    #[error("bus client: {source}")]
    Client {
        #[source]
        source: ClientError,
    },
}

/// Run the publisher end-to-end: connect, handshake, initial
/// bootstrap, poll loop, shutdown-retract. Returns only on clean
/// shutdown (SIGTERM / SIGINT received, facts retracted) or fatal
/// error.
pub async fn run(
    observer: RepoObserver,
    socket_override: Option<PathBuf>,
    poll_interval: Duration,
) -> Result<(), PublisherError> {
    let socket = socket_override.unwrap_or_else(default_socket);
    let identity =
        ActorIdentity::service("git-watcher", Uuid::new_v4()).expect("kebab-case service-id");
    let instance_id = match &identity {
        ActorIdentity::Service { instance_id, .. } => *instance_id,
        _ => unreachable!("ActorIdentity::service returns a Service variant"),
    };

    info!(
        repository = %observer.path().display(),
        socket = %socket.display(),
        poll_interval = ?poll_interval,
        instance = %instance_id,
        "weaver-git-watcher starting",
    );

    // T040: handshake via Client, then split the stream so the
    // reader task can surface server-sent Error frames concurrently
    // with the write path (F3 review fix).
    let client = Client::connect(&socket, "git-watcher")
        .await
        .map_err(|source| PublisherError::BusUnavailable { source })?;
    info!("connected to core; bus protocol handshake complete");

    let (reader, writer_half) = client.stream.into_split();
    let mut writer = BusWriter {
        writer: writer_half,
    };
    let (err_tx, mut err_rx) = mpsc::channel::<ServerSentError>(4);
    let reader_task = tokio::spawn(reader_loop(reader, err_tx));

    // Entity refs for this watcher's publications.
    let repo_entity = repo_entity_ref(observer.path());
    let watcher_entity = watcher_instance_entity_ref(&instance_id);

    // Tracked facts (for retraction on shutdown).
    let mut tracked: HashSet<FactKey> = HashSet::new();
    let mut last: Option<Observation>;

    // Status: started → ready after bootstrap.
    debug!("publishing watcher/status Started");
    publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Started,
    )
    .await?;
    debug!("published Started; observing initial state");

    // T041: initial bootstrap publish.
    let initial = observer
        .observe()
        .map_err(|source| PublisherError::Observer { source })?;
    debug!("observed initial; publishing bootstrap facts");
    publish_observation(
        &mut writer,
        repo_entity,
        observer.path(),
        &identity,
        &initial,
        &mut tracked,
        None,
    )
    .await?;
    debug!("published bootstrap; marking observable=true");
    publish_fact(
        &mut writer,
        FactKey::new(repo_entity, "repo/observable"),
        FactValue::Bool(true),
        &identity,
        None,
        &mut tracked,
    )
    .await?;
    publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Ready,
    )
    .await?;
    info!(
        repo_entity = %repo_entity.as_u64(),
        facts_tracked = tracked.len(),
        "initial bootstrap complete; entering poll loop"
    );
    last = Some(initial);

    // Fail-fast bootstrap check: if the core rejected any of our
    // bootstrap FactAsserts with an authority-conflict, the reader
    // task will have queued a `ServerSentError`. A brief wait here
    // surfaces the conflict before we enter the poll loop — so w2
    // exits immediately with code 3 instead of looping silently.
    if let Ok(Ok(err)) = tokio::time::timeout(Duration::from_millis(250), err_rx.recv())
        .await
        .map(|opt| opt.ok_or(()))
    {
        reader_task.abort();
        return Err(translate_server_error(err));
    }

    // Signal handlers for clean shutdown.
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();

    let mut ticker = interval(poll_interval);
    ticker.tick().await; // burn the immediate first tick

    // Track whether we're in the Degraded state so recovery re-
    // publishes observable=true even when the repo state didn't
    // change across the degraded window (F4 review fix).
    let mut was_degraded = false;

    // T042: poll loop.
    loop {
        select! {
            _ = ticker.tick() => {}
            _ = wait_signal(&mut sigterm), if sigterm.is_some() => {
                info!("SIGTERM received; retracting facts and exiting");
                break;
            }
            _ = wait_signal(&mut sigint), if sigint.is_some() => {
                info!("SIGINT received; retracting facts and exiting");
                break;
            }
            maybe_err = err_rx.recv() => {
                match maybe_err {
                    Some(err) => {
                        reader_task.abort();
                        return Err(translate_server_error(err));
                    }
                    None => {
                        // The reader task exited (likely EOF from the
                        // core). Treat as bus unavailability; surface
                        // via BusUnavailable for exit-code 2.
                        return Err(PublisherError::BusUnavailable {
                            source: ClientError::Codec(CodecError::Io(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "bus connection closed",
                            ))),
                        });
                    }
                }
            }
        }

        // Attempt observation. On error, enter Degraded (T046).
        //
        // F21 review fix: only emit the Degraded lifecycle and
        // `repo/observable=false` on the healthy→degraded
        // transition. Republishing every failed poll during a
        // prolonged outage floods the trace and bus broadcast
        // with identical authoritative assertions; subscribers
        // already observed the first one, and the mutex-invariant
        // fact is still in the store. Subsequent failures stay at
        // debug log level.
        let obs = match observer.observe() {
            Ok(o) => o,
            Err(e) => {
                if was_degraded {
                    debug!(error = %e, "observation still failing; remaining Degraded");
                } else {
                    warn!(error = %e, "observation failed; entering Degraded");
                    was_degraded = true;
                    let _ = publish_watcher_status(
                        &mut writer,
                        watcher_entity,
                        &identity,
                        LifecycleSignal::Degraded,
                    )
                    .await;
                    let _ = publish_fact(
                        &mut writer,
                        FactKey::new(repo_entity, "repo/observable"),
                        FactValue::Bool(false),
                        &identity,
                        None,
                        &mut tracked,
                    )
                    .await;
                }
                continue;
            }
        };

        // F4 review fix: if we were Degraded, publish Ready +
        // observable=true *unconditionally* on the first successful
        // observation — not just inside the `prev != obs` branch.
        if was_degraded {
            debug!("observation recovered from Degraded; publishing Ready + observable=true");
            let _ = publish_watcher_status(
                &mut writer,
                watcher_entity,
                &identity,
                LifecycleSignal::Ready,
            )
            .await;
            let _ = publish_fact(
                &mut writer,
                FactKey::new(repo_entity, "repo/observable"),
                FactValue::Bool(true),
                &identity,
                None,
                &mut tracked,
            )
            .await;
            was_degraded = false;
        }

        if let Some(prev) = &last {
            if prev != &obs {
                // Synthesize a poll-tick event id so transition
                // retract+assert share a causal parent.
                let poll_tick_id = EventId::new(now_ns());
                diff_publish(
                    &mut writer,
                    repo_entity,
                    observer.path(),
                    &identity,
                    prev,
                    &obs,
                    &mut tracked,
                    poll_tick_id,
                )
                .await?;
            }
        }
        last = Some(obs);
    }

    // T047: shutdown — retract all facts this instance published, then
    // emit Unavailable → Stopped.
    shutdown_retract(&mut writer, &identity, &mut tracked).await;
    let _ = publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Unavailable,
    )
    .await;
    let _ = publish_watcher_status(
        &mut writer,
        watcher_entity,
        &identity,
        LifecycleSignal::Stopped,
    )
    .await;
    reader_task.abort();
    debug!("publisher exiting cleanly");
    Ok(())
}

/// Reader task: drains server-sent `BusMessage`s, filters for the
/// `Error` variants that matter to the publisher's control flow, and
/// forwards them over `err_tx`. Exits cleanly on EOF (drops `err_tx`,
/// which wakes the main loop's recv arm with `None`).
async fn reader_loop(mut reader: OwnedReadHalf, err_tx: mpsc::Sender<ServerSentError>) {
    loop {
        match read_message(&mut reader).await {
            Ok(BusMessage::Error(msg)) => {
                let classified = match msg.category.as_str() {
                    "authority-conflict" => ServerSentError::AuthorityConflict {
                        detail: msg.detail.clone(),
                    },
                    "not-owner" => ServerSentError::NotOwner {
                        detail: msg.detail.clone(),
                    },
                    _ => ServerSentError::Other {
                        category: msg.category.clone(),
                        detail: msg.detail.clone(),
                    },
                };
                // Send-best-effort; if the main loop has already
                // torn down, dropping is fine.
                let fatal = matches!(classified, ServerSentError::AuthorityConflict { .. });
                let _ = err_tx.send(classified).await;
                if fatal {
                    return;
                }
            }
            Ok(_) => {
                // Other server-sent messages (SubscribeAck,
                // FactAssert from other publishers, etc.) aren't
                // actionable here; ignore.
            }
            Err(_) => {
                // EOF or codec error. Dropping err_tx here signals
                // the main loop that the connection is gone.
                return;
            }
        }
    }
}

/// Classified server-sent error surfaced from the reader task to the
/// main poll loop. Other categories are forwarded for diagnostics but
/// don't necessarily cause publisher exit.
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
        ServerSentError::Other { category, detail } => PublisherError::Client {
            source: ClientError::Codec(CodecError::Io(std::io::Error::other(format!(
                "server error {category}: {detail}"
            )))),
        },
    }
}

async fn wait_signal(sig: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(s) = sig.as_mut() {
        let _ = s.recv().await;
    } else {
        // If we failed to install the handler, never fire this branch.
        std::future::pending::<()>().await;
    }
}

/// Publish the full bootstrap set for a repository observation:
/// `repo/path`, `repo/dirty`, `repo/head-commit` (if Some), and the
/// single `repo/state/*` variant matching the observation.
async fn publish_observation(
    writer: &mut BusWriter,
    repo_entity: EntityRef,
    repo_path: &Path,
    identity: &ActorIdentity,
    obs: &Observation,
    tracked: &mut HashSet<FactKey>,
    causal_parent: Option<EventId>,
) -> Result<(), PublisherError> {
    publish_fact(
        writer,
        FactKey::new(repo_entity, "repo/path"),
        FactValue::String(repo_path.display().to_string()),
        identity,
        causal_parent,
        tracked,
    )
    .await?;
    publish_fact(
        writer,
        FactKey::new(repo_entity, "repo/dirty"),
        FactValue::Bool(obs.dirty),
        identity,
        causal_parent,
        tracked,
    )
    .await?;
    if let Some(sha) = &obs.head_commit {
        publish_fact(
            writer,
            FactKey::new(repo_entity, "repo/head-commit"),
            FactValue::String(sha.clone()),
            identity,
            causal_parent,
            tracked,
        )
        .await?;
    }
    let (state_attr, state_value) = state_fact(&obs.state);
    publish_fact(
        writer,
        FactKey::new(repo_entity, state_attr),
        state_value,
        identity,
        causal_parent,
        tracked,
    )
    .await?;
    Ok(())
}

fn state_fact(state: &WorkingCopyState) -> (&'static str, FactValue) {
    match state {
        WorkingCopyState::OnBranch { name } => {
            ("repo/state/on-branch", FactValue::String(name.clone()))
        }
        WorkingCopyState::Detached { commit } => {
            ("repo/state/detached", FactValue::String(commit.clone()))
        }
        WorkingCopyState::Unborn {
            intended_branch_name,
        } => (
            "repo/state/unborn",
            FactValue::String(intended_branch_name.clone()),
        ),
    }
}

/// Diff `prev` vs `next` observations and publish only the changed
/// facts. State transitions (discriminator change on `repo/state/*`)
/// emit a retract-then-assert pair with a shared `causal_parent`
/// matching T043 semantics (mutex invariant preserved).
#[allow(clippy::too_many_arguments)] // TODO: refactor into a PublisherCtx { client, identity, tracked } + diff(&self, prev, next, ...) in a follow-up.
async fn diff_publish(
    writer: &mut BusWriter,
    repo_entity: EntityRef,
    repo_path: &Path,
    identity: &ActorIdentity,
    prev: &Observation,
    next: &Observation,
    tracked: &mut HashSet<FactKey>,
    poll_tick_id: EventId,
) -> Result<(), PublisherError> {
    let causal = Some(poll_tick_id);

    // State-variant transition? If so, retract the old variant first.
    if std::mem::discriminant(&prev.state) != std::mem::discriminant(&next.state) {
        let (prev_attr, _) = state_fact(&prev.state);
        retract_fact(
            writer,
            FactKey::new(repo_entity, prev_attr),
            identity,
            causal,
            tracked,
        )
        .await?;
    }
    // Always (re-)assert the current state. If the variant is the same
    // but the payload changed (branch renamed / head shifted), this
    // updates in place.
    let (state_attr, state_value) = state_fact(&next.state);
    publish_fact(
        writer,
        FactKey::new(repo_entity, state_attr),
        state_value,
        identity,
        causal,
        tracked,
    )
    .await?;

    // Dirty + head-commit: (re-)assert on change. No retract needed —
    // these are single-value attributes.
    if prev.dirty != next.dirty {
        publish_fact(
            writer,
            FactKey::new(repo_entity, "repo/dirty"),
            FactValue::Bool(next.dirty),
            identity,
            causal,
            tracked,
        )
        .await?;
    }
    if prev.head_commit != next.head_commit {
        match &next.head_commit {
            Some(sha) => {
                publish_fact(
                    writer,
                    FactKey::new(repo_entity, "repo/head-commit"),
                    FactValue::String(sha.clone()),
                    identity,
                    causal,
                    tracked,
                )
                .await?;
            }
            None => {
                retract_fact(
                    writer,
                    FactKey::new(repo_entity, "repo/head-commit"),
                    identity,
                    causal,
                    tracked,
                )
                .await?;
            }
        }
    }

    // repo/path rarely changes; re-publish if the canonicalized form
    // moves (unlikely but defensive).
    let path_str = repo_path.display().to_string();
    let path_key = FactKey::new(repo_entity, "repo/path");
    if !tracked.contains(&path_key) {
        publish_fact(
            writer,
            path_key.clone(),
            FactValue::String(path_str),
            identity,
            causal,
            tracked,
        )
        .await?;
    }
    Ok(())
}

async fn publish_watcher_status(
    writer: &mut BusWriter,
    watcher_entity: EntityRef,
    identity: &ActorIdentity,
    signal: LifecycleSignal,
) -> Result<(), PublisherError> {
    // `watcher/status` as a string value to match the generic FactValue
    // shape. JSON/CBOR wire stays kebab-case via the LifecycleSignal
    // enum's rename_all.
    let label = match signal {
        LifecycleSignal::Started => "started",
        LifecycleSignal::Ready => "ready",
        LifecycleSignal::Degraded => "degraded",
        LifecycleSignal::Unavailable => "unavailable",
        LifecycleSignal::Restarting => "restarting",
        LifecycleSignal::Stopped => "stopped",
    };
    let key = FactKey::new(watcher_entity, "watcher/status");
    let prov = Provenance::new(identity.clone(), now_ns(), None)
        .expect("ActorIdentity is always well-formed");
    let fact = Fact {
        key: key.clone(),
        value: FactValue::String(label.into()),
        provenance: prov,
    };
    writer
        .send(&BusMessage::FactAssert(fact))
        .await
        .map_err(|source| PublisherError::Client { source })?;
    // We don't track watcher/status for retraction on shutdown — we
    // overwrite it to Stopped instead.
    let _ = key;
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

async fn retract_fact(
    writer: &mut BusWriter,
    key: FactKey,
    identity: &ActorIdentity,
    causal_parent: Option<EventId>,
    tracked: &mut HashSet<FactKey>,
) -> Result<(), PublisherError> {
    let prov = Provenance::new(identity.clone(), now_ns(), causal_parent)
        .expect("ActorIdentity is always well-formed");
    writer
        .send(&BusMessage::FactRetract {
            key: key.clone(),
            provenance: prov,
        })
        .await
        .map_err(|source| PublisherError::Client { source })?;
    tracked.remove(&key);
    Ok(())
}

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

/// Derive a stable `EntityRef` for a watched repository from its
/// canonical path. Hashing keeps the mapping deterministic across
/// watcher invocations on the same repo without requiring a central
/// registry.
fn repo_entity_ref(path: &Path) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // Reserve the high bit so repo entities don't collide with any
    // low-count buffer entities from slice 001. Set bit 63 on.
    let h = hasher.finish() | (1u64 << 63);
    EntityRef::new(h)
}

/// Derive a stable `EntityRef` for the watcher-instance entity (host
/// of `watcher/status`). Uses a distinct high bit from repo entities
/// so traces can distinguish instance entities at a glance.
fn watcher_instance_entity_ref(instance: &Uuid) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    instance.as_bytes().hash(&mut hasher);
    // Reserve bit 62 (distinct from repo's bit 63) — arbitrary but
    // stable.
    let h = hasher.finish() | (1u64 << 62);
    // Clear bit 63 so it doesn't look like a repo entity.
    EntityRef::new(h & !(1u64 << 63))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
