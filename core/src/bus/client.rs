//! Bus client helpers — connect, handshake, and publish.
//!
//! Shared between the TUI (`weaver-tui`) and the `weaver` CLI's one-shot
//! subcommands (`status`, `inspect`). Both need the same Hello → Ready
//! handshake; duplicating it would drift.
//!
//! Slice 001 keeps the helper deliberately small: one `connect` that
//! completes the handshake, plus `publish_event` / `subscribe_ack` / a
//! read helper. Richer reconnect logic is a later-slice concern
//! (`research.md` §open items).

use std::path::Path;

use thiserror::Error;
use tokio::net::UnixStream;

use crate::bus::codec::{CodecError, read_message, write_message};
use crate::types::message::{
    BUS_PROTOCOL_VERSION, BusMessage, EventSubscribePattern, HelloMsg, LifecycleSignal,
    SubscribePattern,
};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to bus socket at {path}: {source}")]
    Connect {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("handshake: expected Lifecycle(Ready), got {got:?}")]
    HandshakeUnexpected { got: BusMessage },

    #[error("subscribe: expected SubscribeAck, got {got:?}")]
    SubscribeAckUnexpected { got: BusMessage },
}

/// A connected, handshaken bus client.
pub struct Client {
    pub stream: UnixStream,
}

impl Client {
    /// Open a Unix-socket connection to the core's bus and complete the
    /// `Hello` → `Lifecycle(Ready)` handshake. `client_kind` identifies
    /// the client for operator observability (e.g., `"tui"`, `"cli"`).
    pub async fn connect(socket: &Path, client_kind: &str) -> Result<Self, ClientError> {
        let mut stream =
            UnixStream::connect(socket)
                .await
                .map_err(|source| ClientError::Connect {
                    path: socket.display().to_string(),
                    source,
                })?;

        let hello = BusMessage::Hello(HelloMsg {
            protocol_version: BUS_PROTOCOL_VERSION,
            client_kind: client_kind.into(),
        });
        write_message(&mut stream, &hello).await?;

        match read_message(&mut stream).await? {
            BusMessage::Lifecycle(LifecycleSignal::Ready) => Ok(Self { stream }),
            got => Err(ClientError::HandshakeUnexpected { got }),
        }
    }

    /// Subscribe to a fact pattern; block until the `SubscribeAck` is
    /// received. Returns the starting sequence number for gap detection
    /// per `contracts/bus-messages.md`.
    pub async fn subscribe(&mut self, pattern: SubscribePattern) -> Result<u64, ClientError> {
        write_message(&mut self.stream, &BusMessage::Subscribe(pattern)).await?;
        match read_message(&mut self.stream).await? {
            BusMessage::SubscribeAck { sequence } => Ok(sequence),
            got => Err(ClientError::SubscribeAckUnexpected { got }),
        }
    }

    /// Subscribe to events matching `pattern`; block until the
    /// `SubscribeAck` is received. Slice 004; mirrors [`Self::subscribe`]
    /// for the lossy-class event channel.
    ///
    /// MUST be called BEFORE the connection is split into reader/writer
    /// halves (the ack is consumed inline). On success the listener has
    /// registered the subscription and any subsequent matching event
    /// will be delivered as a [`BusMessage::Event`] frame on the read
    /// half.
    pub async fn subscribe_events(
        &mut self,
        pattern: EventSubscribePattern,
    ) -> Result<(), ClientError> {
        write_message(&mut self.stream, &BusMessage::SubscribeEvents(pattern)).await?;
        match read_message(&mut self.stream).await? {
            BusMessage::SubscribeAck { .. } => Ok(()),
            got => Err(ClientError::SubscribeAckUnexpected { got }),
        }
    }

    /// Send a [`BusMessage`] on the connection.
    pub async fn send(&mut self, msg: &BusMessage) -> Result<(), ClientError> {
        write_message(&mut self.stream, msg).await?;
        Ok(())
    }

    /// Read one [`BusMessage`] from the connection.
    pub async fn recv(&mut self) -> Result<BusMessage, ClientError> {
        Ok(read_message(&mut self.stream).await?)
    }
}
