//! In-TUI keystroke helpers that publish to the bus.
//!
//! Slice 003 removed the `e`/`c` simulate-edit/simulate-clean
//! keystrokes when `buffer/dirty` authority moved from the
//! `core/dirty-tracking` behavior to the `weaver-buffers` service.
//! In-TUI editing is a slice-004 concern. The `i` (inspect) keystroke
//! remains, dispatching an `InspectRequest` for the first visually-
//! displayed fact (resolved in `render.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWrite;

use weaver_core::bus::codec::{CodecError, write_message};
use weaver_core::types::fact::FactKey;
use weaver_core::types::message::BusMessage;

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

fn wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
