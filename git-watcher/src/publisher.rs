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
use tokio::select;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use weaver_core::bus::client::{Client, ClientError};
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, LifecycleSignal};

use crate::model::WorkingCopyState;
use crate::observer::{Observation, RepoObserver};

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

    // T040: handshake.
    let mut client = Client::connect(&socket, "git-watcher")
        .await
        .map_err(|source| PublisherError::BusUnavailable { source })?;
    info!("connected to core; bus protocol handshake complete");

    // Entity refs for this watcher's publications.
    let repo_entity = repo_entity_ref(observer.path());
    let watcher_entity = watcher_instance_entity_ref(&instance_id);

    // Tracked facts (for retraction on shutdown).
    let mut tracked: HashSet<FactKey> = HashSet::new();
    let mut last: Option<Observation>;

    // Status: started → ready after bootstrap.
    debug!("publishing watcher/status Started");
    publish_watcher_status(
        &mut client,
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
        &mut client,
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
        &mut client,
        FactKey::new(repo_entity, "repo/observable"),
        FactValue::Bool(true),
        &identity,
        None,
        &mut tracked,
    )
    .await?;
    publish_watcher_status(
        &mut client,
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

    // Signal handlers for clean shutdown.
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();

    let mut ticker = interval(poll_interval);
    ticker.tick().await; // burn the immediate first tick

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
        }

        // Attempt observation. On error, enter Degraded (T046).
        let obs = match observer.observe() {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "observation failed; entering Degraded");
                let _ = publish_watcher_status(
                    &mut client,
                    watcher_entity,
                    &identity,
                    LifecycleSignal::Degraded,
                )
                .await;
                let _ = publish_fact(
                    &mut client,
                    FactKey::new(repo_entity, "repo/observable"),
                    FactValue::Bool(false),
                    &identity,
                    None,
                    &mut tracked,
                )
                .await;
                continue;
            }
        };

        if let Some(prev) = &last {
            if prev != &obs {
                // Synthesize a poll-tick event id so transition
                // retract+assert share a causal parent.
                let poll_tick_id = EventId::new(now_ns());
                diff_publish(
                    &mut client,
                    repo_entity,
                    observer.path(),
                    &identity,
                    prev,
                    &obs,
                    &mut tracked,
                    poll_tick_id,
                )
                .await?;
                // If we were Degraded and observation recovered, return
                // to Ready + mark observable.
                let _ = publish_watcher_status(
                    &mut client,
                    watcher_entity,
                    &identity,
                    LifecycleSignal::Ready,
                )
                .await;
                let _ = publish_fact(
                    &mut client,
                    FactKey::new(repo_entity, "repo/observable"),
                    FactValue::Bool(true),
                    &identity,
                    None,
                    &mut tracked,
                )
                .await;
            }
        }
        last = Some(obs);
    }

    // T047: shutdown — retract all facts this instance published, then
    // emit Unavailable → Stopped.
    shutdown_retract(&mut client, &identity, &mut tracked).await;
    let _ = publish_watcher_status(
        &mut client,
        watcher_entity,
        &identity,
        LifecycleSignal::Unavailable,
    )
    .await;
    let _ = publish_watcher_status(
        &mut client,
        watcher_entity,
        &identity,
        LifecycleSignal::Stopped,
    )
    .await;
    debug!("publisher exiting cleanly");
    Ok(())
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
    client: &mut Client,
    repo_entity: EntityRef,
    repo_path: &Path,
    identity: &ActorIdentity,
    obs: &Observation,
    tracked: &mut HashSet<FactKey>,
    causal_parent: Option<EventId>,
) -> Result<(), PublisherError> {
    publish_fact(
        client,
        FactKey::new(repo_entity, "repo/path"),
        FactValue::String(repo_path.display().to_string()),
        identity,
        causal_parent,
        tracked,
    )
    .await?;
    publish_fact(
        client,
        FactKey::new(repo_entity, "repo/dirty"),
        FactValue::Bool(obs.dirty),
        identity,
        causal_parent,
        tracked,
    )
    .await?;
    if let Some(sha) = &obs.head_commit {
        publish_fact(
            client,
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
        client,
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
    client: &mut Client,
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
            client,
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
        client,
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
            client,
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
                    client,
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
                    client,
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
            client,
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
    client: &mut Client,
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
    client
        .send(&BusMessage::FactAssert(fact))
        .await
        .map_err(|source| PublisherError::Client { source })?;
    // We don't track watcher/status for retraction on shutdown — we
    // overwrite it to Stopped instead.
    let _ = key;
    Ok(())
}

async fn publish_fact(
    client: &mut Client,
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
    client
        .send(&BusMessage::FactAssert(fact))
        .await
        .map_err(|source| PublisherError::Client { source })?;
    tracked.insert(key);
    Ok(())
}

async fn retract_fact(
    client: &mut Client,
    key: FactKey,
    identity: &ActorIdentity,
    causal_parent: Option<EventId>,
    tracked: &mut HashSet<FactKey>,
) -> Result<(), PublisherError> {
    let prov = Provenance::new(identity.clone(), now_ns(), causal_parent)
        .expect("ActorIdentity is always well-formed");
    client
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
    client: &mut Client,
    identity: &ActorIdentity,
    tracked: &mut HashSet<FactKey>,
) {
    let keys: Vec<FactKey> = tracked.drain().collect();
    for key in keys {
        let prov = match Provenance::new(identity.clone(), now_ns(), None) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let _ = client
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
