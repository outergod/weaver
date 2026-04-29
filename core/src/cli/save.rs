//! `weaver save` subcommand implementation.
//!
//! Fire-and-forget: parses `<ENTITY>` (path or stringified `EntityRef`),
//! looks up `buffer/version` via the slice-004 in-process inspect
//! library function, mints a UUIDv8 EventId with the per-process
//! User prefix, and dispatches a `BufferSave` event over the bus.
//!
//! See `specs/005-buffer-save/contracts/cli-surfaces.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{IntoDiagnostic, miette};
use tokio::runtime::Builder;

use crate::bus::client::{Client, ClientError};
use crate::cli::args::OutputFormat;
use crate::cli::config::Config;
use crate::cli::errors::{WeaverCliError, render_error};
use crate::provenance::{ActorIdentity, Provenance};
use crate::types::buffer_entity::buffer_entity_ref;
use crate::types::entity_ref::EntityRef;
use crate::types::event::{Event, EventPayload};
use crate::types::fact::{FactKey, FactValue};
use crate::types::ids::{EventId, hash_to_58};
use crate::types::message::{BusMessage, InspectionError};

/// Per-process User-identity UUIDv8 prefix for `weaver save` event
/// minting. Initialised lazily on first emit from a fresh
/// `Uuid::new_v4()` hashed to 58 bits via SipHash (per slice-005
/// §28(a) re-derivation; see `specs/005-buffer-save/research.md`
/// §5 + §12). Producer-restart yields a fresh prefix — acceptable
/// because in-memory traces don't survive listener restart anyway.
fn user_event_prefix() -> u64 {
    use std::sync::OnceLock;
    use uuid::Uuid;
    static PREFIX: OnceLock<u64> = OnceLock::new();
    *PREFIX.get_or_init(|| hash_to_58(&Uuid::new_v4()))
}

/// Resolve `<ENTITY>` to (entity, display-string).
///
/// Per `cli-surfaces.md §weaver save`: if the argument parses as a
/// `u64`, treat as `EntityRef` verbatim (no canonical path is
/// available; pass-through the arg as the display token). Otherwise
/// canonicalise + derive entity via `buffer_entity_ref`.
///
/// Returns `Err(detail)` on path-form canonicalisation failure; the
/// caller renders WEAVER-101 with the detail. u64-form is infallible.
fn resolve_entity(arg: &str) -> Result<(EntityRef, String), String> {
    if let Ok(n) = arg.parse::<u64>() {
        return Ok((EntityRef::new(n), arg.to_string()));
    }
    let canonical =
        std::fs::canonicalize(arg).map_err(|e| format!("cannot canonicalise path {arg}: {e}"))?;
    let entity = buffer_entity_ref(&canonical);
    Ok((entity, canonical.display().to_string()))
}

/// Run `weaver save <ENTITY>` end-to-end.
///
/// Flow per `specs/005-buffer-save/contracts/cli-surfaces.md
/// §weaver save §Pre-dispatch flow`:
///
/// 1. Resolve `<ENTITY>` (auto-detect u64 vs path).
/// 2. Connect to bus.
/// 3. Inspect-lookup `(entity, buffer/version)`:
///    - `FactNotFound` → render WEAVER-SAVE-001 (exit 1).
///    - `Found` with `value: FactValue::U64(version)` → use `version`.
///    - Other shapes → exit 10 (constitutional violation).
/// 4. Construct `Event { payload: BufferSave { entity, version } }`
///    with a UUIDv8 EventId minted via the per-process User-prefix.
/// 5. Dispatch via `BusMessage::Event`; exit 0 (fire-and-forget).
pub fn handle_save(
    entity_arg: String,
    output: OutputFormat,
    socket_override: Option<PathBuf>,
) -> miette::Result<()> {
    // Step 1: resolve <ENTITY>.
    let (entity, entity_display) = match resolve_entity(&entity_arg) {
        Ok(p) => p,
        Err(detail) => {
            let err = WeaverCliError::ParseError {
                message: detail,
                context: Some(format!("weaver save {entity_arg}")),
            };
            render_error(&err, output)?;
            std::process::exit(err.exit_code());
        }
    };

    let cfg = Config::from_cli(socket_override);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        // Step 2: connect.
        let mut client = match Client::connect(&cfg.socket_path, "weaver-save").await {
            Ok(c) => c,
            Err(ClientError::Connect {
                path: socket_path,
                source,
            }) => {
                let err = WeaverCliError::CoreUnavailable {
                    message: format!("core not reachable at {socket_path}: {source}"),
                    context: Some(format!("weaver save {entity_arg}")),
                };
                render_error(&err, output)?;
                std::process::exit(err.exit_code());
            }
            Err(e) => return Err(miette!("{e}")),
        };

        // Step 3: inspect-lookup buffer/version.
        let key = FactKey::new(entity, "buffer/version");
        let request_id = next_request_id();
        client
            .send(&BusMessage::InspectRequest {
                request_id,
                fact: key,
            })
            .await
            .map_err(|e| miette!("{e}"))?;
        let response = loop {
            match client.recv().await.map_err(|e| miette!("{e}"))? {
                BusMessage::InspectResponse {
                    request_id: rid,
                    result,
                } if rid == request_id => break result,
                // Defensive: drop spurious frames from a non-subscribed
                // connection. Should not happen on this code path.
                _ => continue,
            }
        };
        let version = match response {
            Err(InspectionError::FactNotFound) => {
                let err = WeaverCliError::BufferNotOpenedSave {
                    entity_arg: entity_display,
                    entity: entity.as_u64(),
                    context: Some(format!("weaver save {entity_arg}")),
                };
                render_error(&err, output)?;
                std::process::exit(err.exit_code());
            }
            Err(InspectionError::NoProvenance) => {
                let err = WeaverCliError::ProtocolError {
                    message: format!(
                        "buffer/version exists for entity {} but has no provenance",
                        entity.as_u64()
                    ),
                    context: Some("weaver save inspect-lookup".into()),
                };
                render_error(&err, output)?;
                std::process::exit(10);
            }
            Ok(detail) => match detail.value {
                FactValue::U64(v) => v,
                other => {
                    let err = WeaverCliError::ProtocolError {
                        message: format!(
                            "buffer/version (entity {}) expected U64 but got {other:?}",
                            entity.as_u64()
                        ),
                        context: Some("weaver save inspect-lookup".into()),
                    };
                    render_error(&err, output)?;
                    std::process::exit(10);
                }
            },
        };

        // Step 4: construct event with UUIDv8 EventId. ActorIdentity::User
        // identifies the CLI emitter; the per-process User-prefix
        // partitions the producer's UUIDv8 namespace away from any
        // Service producer's prefix.
        let now = now_ns();
        let provenance = Provenance::new(ActorIdentity::User, now, None)
            .expect("ActorIdentity::User has no fields to validate");
        let event = Event {
            id: EventId::mint_v8(user_event_prefix(), now),
            name: "buffer/save".into(),
            target: Some(entity),
            payload: EventPayload::BufferSave { entity, version },
            provenance,
        };

        // Step 5: dispatch + exit 0 (fire-and-forget per FR-012). The
        // CLI does NOT wait for the service to apply; the operator
        // observes the buffer/dirty=false flip on the TUI / via
        // `weaver inspect --why` post-dispatch.
        client
            .send(&BusMessage::Event(event))
            .await
            .map_err(|e| miette!("{e}"))?;
        Ok(())
    })
}

/// Monotonic per-process inspect request-id counter. One
/// `weaver save` invocation issues exactly one InspectRequest, but
/// the counter is process-scoped to keep the shape consistent with
/// `cli::edit::next_request_id` and `cli::inspect::next_request_id`.
fn next_request_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
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

    /// Use a process-pid + wall-clock unique filename in
    /// `std::env::temp_dir()` rather than a `tempfile` dev-dep —
    /// matches `cli::edit`'s test convention (tempfile is not a
    /// `core` dev-dep). Best-effort cleanup via `remove_file` at the
    /// end of each test.
    fn unique_temp_path(label: &str) -> PathBuf {
        let pid = std::process::id();
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("weaver-save-test-{label}-{pid}-{tick}.txt"))
    }

    #[test]
    fn resolve_entity_parses_u64_form() {
        let (entity, display) = resolve_entity("4611686018427387946").expect("u64 parse");
        assert_eq!(entity.as_u64(), 4611686018427387946);
        assert_eq!(display, "4611686018427387946");
    }

    #[test]
    fn resolve_entity_canonicalises_path_form() {
        let path = unique_temp_path("canonicalises-path-form");
        std::fs::write(&path, b"x").expect("write fixture");
        let canonical = std::fs::canonicalize(&path).expect("canonicalize");
        let arg = canonical.to_string_lossy().to_string();
        let (entity, display) = resolve_entity(&arg).expect("path resolve");
        assert_eq!(entity, buffer_entity_ref(&canonical));
        assert_eq!(display, canonical.display().to_string());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_entity_rejects_nonexistent_path() {
        let missing = "/definitely/not/a/real/path/weaver-save-test";
        let err = resolve_entity(missing).expect_err("missing path must fail");
        assert!(err.contains("cannot canonicalise"));
        assert!(err.contains(missing));
    }

    #[test]
    fn resolve_entity_u64_max_round_trips() {
        let s = u64::MAX.to_string();
        let (entity, display) = resolve_entity(&s).expect("u64::MAX parse");
        assert_eq!(entity.as_u64(), u64::MAX);
        assert_eq!(display, s);
    }
}
