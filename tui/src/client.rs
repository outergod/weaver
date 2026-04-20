//! TUI bus client — connect, handshake, subscribe, receive messages.
//!
//! Slice 001 Phase 2 implements the connection + Hello + Subscribe
//! sequence. Disconnect detection (FR-010) and fact rendering land
//! in Phase 3 (T071, T047).

use std::path::Path;

use miette::miette;
use thiserror::Error;
use tokio::net::UnixStream;

use weaver_core::bus::codec::{CodecError, read_message, write_message};
use weaver_core::types::message::{
    BUS_PROTOCOL_VERSION, BusMessage, HelloMsg, LifecycleSignal, SubscribePattern,
};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to bus socket: {0}")]
    Connect(#[source] std::io::Error),

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("handshake: expected Lifecycle(Ready), got {got:?}")]
    HandshakeUnexpected { got: BusMessage },

    #[error("handshake: expected SubscribeAck, got {got:?}")]
    SubscribeAckUnexpected { got: BusMessage },
}

/// A connected, subscribed bus client.
pub struct Client {
    pub stream: UnixStream,
    pub starting_sequence: u64,
}

/// Connect, handshake, and subscribe to the given pattern.
///
/// Returns the stream with the connection past the handshake, ready
/// for the caller to read messages from. Slice 001 Phase 2 subscribes
/// to `buffer/` by default.
pub async fn connect(socket: &Path, pattern: SubscribePattern) -> Result<Client, ClientError> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(ClientError::Connect)?;

    // Send Hello.
    let hello = BusMessage::Hello(HelloMsg {
        protocol_version: BUS_PROTOCOL_VERSION,
        client_kind: "tui".into(),
    });
    write_message(&mut stream, &hello).await?;

    // Expect Lifecycle(Ready).
    let first = read_message(&mut stream).await?;
    match first {
        BusMessage::Lifecycle(LifecycleSignal::Ready) => {}
        other => return Err(ClientError::HandshakeUnexpected { got: other }),
    }

    // Subscribe.
    write_message(&mut stream, &BusMessage::Subscribe(pattern)).await?;
    let ack = read_message(&mut stream).await?;
    let starting_sequence = match ack {
        BusMessage::SubscribeAck { sequence } => sequence,
        other => return Err(ClientError::SubscribeAckUnexpected { got: other }),
    };

    Ok(Client {
        stream,
        starting_sequence,
    })
}

/// Convenience: connect with the default `buffer/*` subscription
/// pattern and surface any error as a miette diagnostic.
pub async fn connect_default(socket: &Path) -> miette::Result<Client> {
    connect(socket, SubscribePattern::FamilyPrefix("buffer/".into()))
        .await
        .map_err(|e| miette!("{e}"))
}
