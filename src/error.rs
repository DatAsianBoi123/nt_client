//! `NetworkTables` client error types.

use tokio_tungstenite::tungstenite;

/// Error that means the `NetworkTables` connection was closed.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Hash)]
#[error("the ws connection was closed")]
pub struct ConnectionClosedError;

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

