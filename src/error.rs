//! `NetworkTables` client error types.

/// Error that means the `NetworkTables` connection was closed.
#[derive(thiserror::Error, Debug)]
#[error("the ws connection was closed")]
pub struct ConnectionClosedError;

