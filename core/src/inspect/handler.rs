//! Bus-level inspection handler — resolves an `InspectRequest` to an
//! `InspectionDetail` or an `InspectionError` against the current fact
//! space and trace store.
//!
//! Slice 001 Phase 4 shape:
//!
//! * Input: a [`FactSpaceSnapshot`] (cheap `Arc` clone) and an
//!   immutable reference to a [`TraceStore`], plus the [`FactKey`] to
//!   inspect.
//! * Output: [`InspectionDetail`] on success, or:
//!   - `FactNotFound` when the fact is not currently asserted.
//!   - `NoProvenance` (defensive, L2 P11) when the fact is asserted
//!     but no `BehaviorFired` entry points at it — unreachable in
//!     slice 001 where every assertion flows through the behavior
//!     dispatcher, but kept as a guard for future external producers.
//!
//! Reference: `specs/001-hello-fact/contracts/bus-messages.md` §
//! `InspectRequest / InspectResponse`.

use crate::fact_space::FactSpaceSnapshot;
use crate::trace::store::TraceStore;
use crate::types::fact::FactKey;
use crate::types::message::{InspectionDetail, InspectionError};

/// Core inspection routine. Pure with respect to its inputs — no I/O,
/// no mutation. The listener acquires the snapshot + trace lock before
/// calling, then drops the locks before writing the response.
pub fn inspect_fact(
    snapshot: &FactSpaceSnapshot,
    trace: &TraceStore,
    key: &FactKey,
) -> Result<InspectionDetail, InspectionError> {
    if !snapshot.contains_key(key) {
        return Err(InspectionError::FactNotFound);
    }
    let Some((source_event, asserting_behavior, trace_sequence)) = trace.fact_inspection(key)
    else {
        return Err(InspectionError::NoProvenance);
    };
    let entry = trace
        .get(trace_sequence)
        .ok_or(InspectionError::NoProvenance)?;
    Ok(InspectionDetail {
        source_event,
        asserting_behavior,
        asserted_at_ns: entry.timestamp_ns,
        trace_sequence: trace_sequence.as_u64(),
    })
}
