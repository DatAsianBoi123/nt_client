//! `NetworkTables` client error types.

use std::error::Error;

use tokio::task::JoinError;
use tokio_tungstenite::tungstenite;

/// Error that means the `NetworkTables` connection was closed.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Hash)]
#[error("the ws connection was closed")]
pub struct ConnectionClosedError;

/// Errors than can occur when connecting to a `NetworkTables` server.
#[derive(thiserror::Error, Debug)]
pub enum ConnectError {
    /// An error occurred with the websocket.
    #[error("websocket error: {0}")]
    WebsocketError(#[from] tungstenite::Error),

    /// An error occurred when joining multiple tasks together.
    #[error(transparent)]
    Join(#[from] JoinError),

    /// An error occurred when pinging the server.
    #[error(transparent)]
    Ping(#[from] PingError),

    /// An error occurred when updating client time.
    #[error(transparent)]
    UpdateTime(#[from] UpdateTimeError),

    /// An error occurred when sending messages to the server.
    #[error(transparent)]
    SendMessage(#[from] SendMessageError),

    /// An error ocurred when receiving messages from the server.
    #[error(transparent)]
    ReceiveMessage(#[from] ReceiveMessageError),
}

/// Errors that can occur when pinging the `NetworkTables` server.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PingError {
    /// The `NetworkTables` connection was closed.
    ///
    /// Normally, this should never happen and may turn into a panic in the future.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),

    /// The server didn't respond with a pong within the timeout specified in
    /// [`NewClientOptions`][`crate::NewClientOptions`].
    #[error("a pong response wasn't received within the timeout")]
    PongTimeout,
}

/// Errors that can occur when updating client time.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UpdateTimeError {
    /// The `NetworkTables` connection was closed.
    ///
    /// Normally, this should never happen and may turn into a panic in the future.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),

    /// Client time has overflowed as a result of the client being alive for too long.
    // TODO: handle time overflow
    #[error("client time has overflowed")]
    TimeOverflow,
}

/// Errors that can occur when sending messages to the `NetworkTables` server.
#[derive(thiserror::Error, Debug)]
pub enum SendMessageError {
    /// The `NetworkTables` connection was closed.
    ///
    /// Normally, this should never happen and may turn into a panic in the future.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),

    /// A message failed to serialize into a JSON text message.
    ///
    /// This shouldn't normally happen and may turn into a panic in the future.
    #[error(transparent)]
    FailedSerializingJson(#[from] serde_json::Error),

    /// A message failed to serialize into a MessagePack binary message.
    ///
    /// This shouldn't normally happen and may turn into a panic in the future.
    #[error(transparent)]
    FailedSerializingMsgPack(#[from] rmp_serde::encode::Error),
}

/// Errors that can occur when receiving messages from the `NetworkTables` server.
#[derive(thiserror::Error, Debug)]
pub enum ReceiveMessageError {
    /// An error has occurred with the websocket.
    #[error(transparent)]
    WebSocket(#[from] tungstenite::Error),

    /// The `NetworkTables` connection was closed.
    ///
    /// Normally, this should never happen and may turn into a panic in the future.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),

    /// A JSON text message failed to deserialize into data.
    #[error(transparent)]
    FailedDeserializingJson(#[from] serde_json::Error),

    /// A MessagePack binary message failed to deserialize into data.
    #[error(transparent)]
    FailedDeserializingMsgPack(#[from] rmp_serde::decode::Error),
}

/// Errors that can occur when converting an [`NTAddr`] to an [`Ipv4Addr`]
///
/// [`NTAddr`]: crate::NTAddr
/// [`Ipv4Addr`]: std::net::Ipv4Addr
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum IntoAddrError {
    /// The team number is greater than 25599.
    #[error("team number {0} is greater than 25599")]
    InvalidTeamNumber(u16),
}

/// A potentially fatal error that occurs after reconnecting to a `NetworkTables` server.
///
/// This is used when creating a reconnect handler via [`crate::reconnect`], in which a
/// [`Fatal`] variant indicates that no attempt to reconnect should be made,
/// while a [`Nonfatal`] variant indicates that a reconnect attempt will be made.
///
/// A [`From<ConnectError>`][`From`] trait is implemented, constructing a [`Fatal`] variant if and only
/// if it is a [`ConnectError::WebsocketError`] ([`tungstenite::error::Error::Io`]), otherwise it
/// constructs a [`Nonfatal`] variant.
///
/// [`Fatal`]: ReconnectError::Fatal
/// [`Nonfatal`]: ReconnectError::Nonfatal
#[derive(thiserror::Error, Debug)]
pub enum ReconnectError {
    /// An unrecoverable error has occurred and no attempt to reconnect should be made.
    #[error(transparent)]
    Fatal(Box<dyn Error + Send + Sync>),

    /// A recoverable error has occurred and reconnection will be attempted.
    #[error(transparent)]
    Nonfatal(Box<dyn Error + Send + Sync>),
}

impl From<ConnectError> for ReconnectError {
    fn from(value: ConnectError) -> Self {
        match value {
            ConnectError::WebsocketError(tungstenite::Error::Io(error)) => Self::Fatal(error.into()),
            err => Self::Nonfatal(err.into()),
        }
    }
}

