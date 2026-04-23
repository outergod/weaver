//! Bus client: publishes `buffer/*` and `watcher/status` facts, owns
//! the per-buffer poll loop, manages the per-invocation
//! `ActorIdentity::Service` identity, and handles clean-shutdown
//! retraction and bus-EOF exit paths.
//!
//! Slice 003 builds this out across several commits. The C10 skeleton
//! shipped here wires up the structural plumbing shared with slice-002's
//! `weaver-git-watcher` publisher:
//!
//! - Service-identity construction.
//! - Connection + handshake via [`weaver_core::bus::client::Client`].
//! - Post-handshake stream split so a reader task can surface
//!   server-sent `Error` frames concurrently with the write path (the
//!   F3 review pattern from slice 002).
//! - SIGTERM / SIGINT awareness with clean (empty) exit.
//!
//! Bootstrap (T030–T032), poll loop (T033–T036), and shutdown-retract
//! (T037–T038) layer onto this scaffold in subsequent C11–C13 commits.

use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use tokio::net::unix::OwnedReadHalf;
use tokio::select;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use weaver_core::bus::client::{Client, ClientError};
use weaver_core::bus::codec::{CodecError, read_message};
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::message::BusMessage;

/// Kebab-case service-id used in Hello / ActorIdentity / inspect
/// rendering, per `contracts/cli-surfaces.md` and Amendment 5.
const SERVICE_ID: &str = "weaver-buffers";

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
/// Current scope (C10 scaffold): connect + handshake + spawn reader
/// task + idle on SIGTERM / SIGINT. Returns `Ok(())` on clean signal,
/// [`PublisherError::AuthorityConflict`] / [`PublisherError::Client`]
/// on server-sent error frames, or [`PublisherError::BusUnavailable`]
/// on connect failure or reader EOF.
///
/// `paths` and `poll_interval` are carried through for logging; they
/// become load-bearing in C11 (bootstrap) and C12 (poll loop).
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

    info!(
        socket = %socket.display(),
        poll_interval = ?poll_interval,
        instance = %instance_id,
        buffers = paths.len(),
        "weaver-buffers starting",
    );
    for p in &paths {
        debug!(path = %p.display(), "opening buffer (deferred to C11)");
    }

    let client = Client::connect(&socket, SERVICE_ID)
        .await
        .map_err(|source| PublisherError::BusUnavailable { source })?;
    info!("connected to core; bus protocol handshake complete");

    let (reader, _writer_half) = client.stream.into_split();
    let (err_tx, mut err_rx) = mpsc::channel::<ServerSentError>(4);
    let reader_task = tokio::spawn(reader_loop(reader, err_tx));

    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();

    let outcome = select! {
        _ = wait_signal(&mut sigterm), if sigterm.is_some() => {
            info!("SIGTERM received; shutting down");
            Ok(())
        }
        _ = wait_signal(&mut sigint), if sigint.is_some() => {
            info!("SIGINT received; shutting down");
            Ok(())
        }
        maybe_err = err_rx.recv() => {
            match maybe_err {
                Some(err) => Err(translate_server_error(err)),
                None => Err(bus_closed_error()),
            }
        }
    };

    reader_task.abort();
    outcome
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
}
