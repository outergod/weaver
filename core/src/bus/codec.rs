//! Bus codec — length-prefixed CBOR framing.
//!
//! Wire format (per `specs/001-hello-fact/contracts/bus-messages.md`):
//!
//! ```text
//! ┌─────────────────┬───────────────────────────────────────────┐
//! │ length (u32 BE) │ CBOR-encoded BusMessage                   │
//! └─────────────────┴───────────────────────────────────────────┘
//! ```
//!
//! Frames larger than [`MAX_FRAME_SIZE`] are rejected with
//! [`CodecError::FrameTooLarge`].

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::types::message::BusMessage;

/// Maximum frame payload in bytes (64 KiB).
pub const MAX_FRAME_SIZE: usize = 64 * 1024;

/// Headroom reserved between [`MAX_FRAME_SIZE`] and the largest
/// `BusMessage::Event` payload that ingests cleanly. The same `Event`
/// envelope is later wrapped in `BusMessage::EventInspectResponse`
/// (slice-004 `weaver inspect --why`) — that wrapper carries an extra
/// `request_id: u64` plus a `Result<Event, EventInspectionError>`
/// discriminator, costing ~10–20 CBOR bytes today. Without this
/// margin, an `Event` ingested at exactly [`MAX_FRAME_SIZE`] would
/// fail [`write_message`] on the response side, killing the inspect
/// connection.
///
/// 256 bytes leaves ≥12× headroom over the current overhead and
/// absorbs future field additions to `EventInspectResponse` without
/// re-tightening the ingest limit.
pub const RESPONSE_WRAPPER_HEADROOM: usize = 256;

/// Maximum size of a `BusMessage::Event` envelope at ingest. Smaller
/// than [`MAX_FRAME_SIZE`] by [`RESPONSE_WRAPPER_HEADROOM`] so that
/// the same `Event`, when re-wrapped as `BusMessage::EventInspectResponse`
/// during a `weaver inspect --why` walkback, still fits within the
/// codec's frame limit. Used by `weaver edit` / `weaver edit-json`
/// pre-dispatch size checks.
pub const MAX_EVENT_INGEST_FRAME: usize = MAX_FRAME_SIZE - RESPONSE_WRAPPER_HEADROOM;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("frame too large: {size} bytes (max {max})")]
    FrameTooLarge { size: usize, max: usize },

    #[error("CBOR encode error: {0}")]
    Encode(String),

    #[error("CBOR decode error: {0}")]
    Decode(String),
}

/// Encode a message as a length-prefixed CBOR frame and write it.
pub async fn write_message<W>(writer: &mut W, msg: &BusMessage) -> Result<(), CodecError>
where
    W: AsyncWrite + Unpin,
{
    let mut payload = Vec::new();
    ciborium::into_writer(msg, &mut payload).map_err(|e| CodecError::Encode(e.to_string()))?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge {
            size: payload.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    let len = u32::try_from(payload.len()).map_err(|_| CodecError::FrameTooLarge {
        size: payload.len(),
        max: MAX_FRAME_SIZE,
    })?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one length-prefixed CBOR frame and decode it as a message.
pub async fn read_message<R>(reader: &mut R) -> Result<BusMessage, CodecError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_SIZE,
        });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    ciborium::from_reader(payload.as_slice()).map_err(|e| CodecError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::{BUS_PROTOCOL_VERSION, BusMessage, HelloMsg};

    #[tokio::test]
    async fn round_trip_hello() {
        let msg = BusMessage::Hello(HelloMsg {
            protocol_version: BUS_PROTOCOL_VERSION,
            client_kind: "tui".into(),
        });
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();

        let mut cursor = buf.as_slice();
        let back = read_message(&mut cursor).await.unwrap();
        assert_eq!(msg, back);
    }

    #[tokio::test]
    async fn rejects_oversized_frame_on_read() {
        let mut buf: Vec<u8> = Vec::new();
        let oversized_len = (MAX_FRAME_SIZE + 1) as u32;
        buf.extend_from_slice(&oversized_len.to_be_bytes());
        // Don't bother filling the body; read_exact will read past it but
        // the size check happens first.
        buf.extend_from_slice(&[0u8; 16]);

        let mut cursor = buf.as_slice();
        let err = read_message(&mut cursor).await.unwrap_err();
        assert!(matches!(err, CodecError::FrameTooLarge { .. }));
    }
}
