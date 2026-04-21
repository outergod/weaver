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

    // Behavior-authored fact (slice-001 path): the behavior dispatcher
    // recorded a `BehaviorFired` entry whose asserted-set contains
    // this key.
    if let Some((source_event, asserting_behavior, trace_sequence)) = trace.fact_inspection(key) {
        let entry = trace
            .get(trace_sequence)
            .ok_or(InspectionError::NoProvenance)?;
        return Ok(InspectionDetail::behavior(
            source_event,
            asserting_behavior,
            entry.timestamp_ns,
            trace_sequence.as_u64(),
        ));
    }

    // Service-authored fact (slice-002): no behavior firing linked the
    // fact, but the fact itself is currently asserted with structured
    // provenance. Walk back to the assert's trace entry for timestamp +
    // sequence.
    let trace_sequence = trace
        .find_fact_assert(key)
        .ok_or(InspectionError::NoProvenance)?;
    let entry = trace
        .get(trace_sequence)
        .ok_or(InspectionError::NoProvenance)?;
    let asserted_at_ns = entry.timestamp_ns;
    // Re-read the fact from the trace-entry payload for the exact
    // provenance of the asserting call (defensive — the snapshot's
    // provenance is the same object, but trace is authoritative).
    let (source_event_opt, source) = match &entry.payload {
        TracePayload::FactAsserted { fact: f } => {
            (f.provenance.causal_parent, &f.provenance.source)
        }
        _ => (fact.provenance.causal_parent, &fact.provenance.source),
    };
    let source_event = source_event_opt.unwrap_or(EventId::ZERO);
    match source {
        ActorIdentity::Service {
            service_id,
            instance_id,
        } => Ok(InspectionDetail::service(
            source_event,
            service_id.clone(),
            *instance_id,
            asserted_at_ns,
            trace_sequence.as_u64(),
        )),
        // Core / Tui / User / Host / Agent / Behavior-without-trace-index:
        // render the opaque shape. A `Behavior` variant without a
        // matching BehaviorFired entry is an anomaly; surfacing the
        // generic shape is honest rather than fabricating a Behavior
        // detail that has no backing trace.
        _ => Ok(InspectionDetail::opaque(
            source_event,
            asserted_at_ns,
            trace_sequence.as_u64(),
        )),
    }
}
