//! In-TUI keystroke handlers — publish `buffer/edited` / `buffer/cleaned`
//! events to the bus via the shared [`weaver_core::bus::client`] writer
//! half.
//!
//! Slice 001 Phase 3 ships:
//! * `e` — submit a `BufferEdited` event for the synthetic buffer.
//! * `c` — submit a `BufferCleaned` event for the synthetic buffer.
//!
//! The `i` (inspect) keystroke lands with T055 in Phase 4.

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWrite;

use weaver_core::bus::codec::{CodecError, write_message};
use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;
use weaver_core::types::message::BusMessage;

/// The kind of simulated event triggered by a keystroke.
#[derive(Copy, Clone, Debug)]
pub enum SimulateKind {
    Edit,
    Clean,
}

impl SimulateKind {
    fn name(self) -> &'static str {
        match self {
            SimulateKind::Edit => "buffer/edited",
            SimulateKind::Clean => "buffer/cleaned",
        }
    }

    fn payload(self) -> EventPayload {
        match self {
            SimulateKind::Edit => EventPayload::BufferEdited,
            SimulateKind::Clean => EventPayload::BufferCleaned,
        }
    }
}

/// Publish a simulated buffer event on the bus via `writer`.
///
/// The event carries `SourceId::Tui` provenance per the data model and
/// uses a wall-clock-nanosecond `EventId` to avoid collisions across
/// rapid keystrokes.
pub async fn publish<W>(
    writer: &mut W,
    kind: SimulateKind,
    target: EntityRef,
) -> Result<EventId, CodecError>
where
    W: AsyncWrite + Unpin,
{
    let now_ns = wall_ns();
    let event_id = EventId::new(now_ns);
    let event = Event {
        id: event_id,
        name: kind.name().into(),
        target: Some(target),
        payload: kind.payload(),
        provenance: Provenance::new(SourceId::Tui, now_ns, None)
            .expect("Tui source is never rejected"),
    };
    write_message(writer, &BusMessage::Event(event)).await?;
    Ok(event_id)
}

fn wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Send an `InspectRequest` for `fact` on `writer`. Returns the
/// request id so the render layer can correlate with a later
/// `InspectResponse` received through the reader task.
pub async fn inspect<W>(writer: &mut W, fact: FactKey) -> Result<u64, CodecError>
where
    W: AsyncWrite + Unpin,
{
    let request_id = wall_ns();
    write_message(writer, &BusMessage::InspectRequest { request_id, fact }).await?;
    Ok(request_id)
}
