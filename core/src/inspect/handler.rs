//! Bus-level inspection handler — resolves an `InspectRequest` to an
//! `InspectionDetail` or an `InspectionError` against the current fact
//! space and trace store.
//!
//! Slice 002 shape (extending slice 001):
//!
//! * Input: a [`FactSpaceSnapshot`] (cheap `Arc` clone) and an
//!   immutable reference to a [`TraceStore`], plus the [`FactKey`] to
//!   inspect.
//! * Output: [`InspectionDetail`] on success — one of:
//!   - **Behavior-authored** (via `TraceStore::fact_inspection`):
//!     slice-001 path; the fact was asserted during a registered
//!     behavior's firing. Detail names the behavior + source event.
//!   - **Service-authored**: the fact was published via a bare
//!     `FactAssert` by a service (slice 002, e.g. `weaver-git-watcher`).
//!     Detail names the `ActorIdentity::Service` components
//!     (`service-id`, `instance-id`) and the fact's `causal_parent`
//!     event if any.
//!   - **Opaque**: the fact's originating actor is neither `Behavior`
//!     nor `Service` (reserved variants `User` / `Host` / `Agent`, or
//!     `Core` / `Tui`). Detail renders source-event + trace-sequence
//!     only; identity is still observable via the trace.
//! * Error: `FactNotFound` when the fact is not currently asserted;
//!   `NoProvenance` (defensive) if the trace store cannot locate the
//!   assert entry for a currently-asserted fact.
//!
//! Reference: `specs/002-git-watcher-actor/contracts/bus-messages.md`
//! and `specs/002-git-watcher-actor/contracts/cli-surfaces.md`.

use crate::fact_space::FactSpaceSnapshot;
use crate::provenance::ActorIdentity;
use crate::trace::entry::TracePayload;
use crate::trace::store::TraceStore;
use crate::types::fact::FactKey;
use crate::types::ids::EventId;
use crate::types::message::{InspectionDetail, InspectionError};

/// Core inspection routine. Pure with respect to its inputs — no I/O,
/// no mutation. The listener acquires the snapshot + trace lock before
/// calling, then drops the locks before writing the response.
pub fn inspect_fact(
    snapshot: &FactSpaceSnapshot,
    trace: &TraceStore,
    key: &FactKey,
) -> Result<InspectionDetail, InspectionError> {
    let Some(fact) = snapshot.get(key) else {
        return Err(InspectionError::FactNotFound);
    };

    // F23 review fix: derive the inspection shape from the LIVE fact's
    // provenance, not from the `fact_inspection` behavior index.
    // The behavior index is only cleared on retraction, so if a
    // behavior asserted K and a service later overwrote K, the
    // index still points at the behavior — handing back stale
    // attribution. The live fact's `provenance.source` is always
    // current (overwrites replace it), so it's the authoritative
    // source for deciding which `InspectionDetail` variant to
    // return.
    match &fact.provenance.source {
        ActorIdentity::Behavior { .. } => {
            // Behavior-authored (slice-001 path): use the trace
            // index to recover the triggering event + behavior id
            // that were recorded in the matching `BehaviorFired`
            // entry. Because the live fact confirms behavior
            // authorship, the index must be current here.
            let (source_event, asserting_behavior, trace_sequence) = trace
                .fact_inspection(key)
                .ok_or(InspectionError::NoProvenance)?;
            let entry = trace
                .get(trace_sequence)
                .ok_or(InspectionError::NoProvenance)?;
            Ok(InspectionDetail::behavior(
                source_event,
                asserting_behavior,
                entry.timestamp_ns,
                trace_sequence.as_u64(),
                fact.value.clone(),
            ))
        }
        ActorIdentity::Service {
            service_id,
            instance_id,
        } => {
            let (asserted_at_ns, trace_sequence, source_event) =
                resolve_live_assert(trace, key, fact)?;
            Ok(InspectionDetail::service(
                source_event,
                service_id.clone(),
                *instance_id,
                asserted_at_ns,
                trace_sequence.as_u64(),
                fact.value.clone(),
            ))
        }
        // Core / Tui / User / Host / Agent: the actor kind is the
        // identity for this slice (T064 + T067 review direction —
        // `asserting_kind` is the always-present discriminator;
        // richer payload for reserved variants defers to the slice
        // that actually emits them). The kind label flows straight
        // from ActorIdentity so adding new variants later
        // automatically propagates without touching this match.
        other => {
            let (asserted_at_ns, trace_sequence, source_event) =
                resolve_live_assert(trace, key, fact)?;
            Ok(InspectionDetail::kind_only(
                other.kind_label(),
                source_event,
                asserted_at_ns,
                trace_sequence.as_u64(),
                fact.value.clone(),
            ))
        }
    }
}

/// Resolve the live-assert trace entry's timestamp + sequence +
/// causal-parent for a currently-asserted fact. The trace entry is
/// the authoritative source for provenance detail; the snapshot's
/// `Fact` is a safety net if the trace payload is unexpectedly
/// non-`FactAsserted` (shouldn't happen — `find_fact_assert` only
/// returns sequences of `FactAsserted` payloads).
fn resolve_live_assert(
    trace: &TraceStore,
    key: &FactKey,
    fact: &crate::types::fact::Fact,
) -> Result<(u64, crate::trace::entry::TraceSequence, EventId), InspectionError> {
    let trace_sequence = trace
        .find_fact_assert(key)
        .ok_or(InspectionError::NoProvenance)?;
    let entry = trace
        .get(trace_sequence)
        .ok_or(InspectionError::NoProvenance)?;
    let source_event_opt = match &entry.payload {
        TracePayload::FactAsserted { fact: f } => f.provenance.causal_parent,
        _ => fact.provenance.causal_parent,
    };
    Ok((
        entry.timestamp_ns,
        trace_sequence,
        source_event_opt.unwrap_or(EventId::nil()),
    ))
}
