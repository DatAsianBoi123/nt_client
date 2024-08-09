//! Named data channels.
//!
//! Topics have a fixed data type and can be subscribed and published to.

use std::{sync::Arc, time::Duration};

use tokio::sync::RwLock;

use crate::{data::{r#type::NetworkTableData, Properties, SubscriptionOptions}, publish::{NewPublisherError, Publisher}, subscribe::{NewSubscriberError, Subscriber}, NTClientSender, NTServerSender, NetworkTablesTime};

/// Represents a `NetworkTables` topic.
///
/// A topic is a named data channel with a fixed data type that can be subscribed and published to.
/// The intended method to obtain one of these is to use the [`Client::topic`] method.
///
/// [`Client::topic`]: crate::Client::topic
#[derive(Clone)]
pub struct Topic {
    name: String,
    time: Arc<RwLock<NetworkTablesTime>>,
    response_timeout: Duration,
    send_ws: NTServerSender,
    recv_ws: NTClientSender,
}

impl Topic {
    // NOTE: pub(super) or just pub?
    pub(super) fn new(
        name: String,
        time: Arc<RwLock<NetworkTablesTime>>,
        response_timeout: Duration,
        send_ws: NTServerSender,
        recv_ws: NTClientSender,
    ) -> Self {
        Self { name, time, response_timeout, send_ws, recv_ws }
    }

    /// Publishes to this topic with the data type `T`.
    ///
    /// # Note
    /// This method requires the [`Client`] websocket connection to already be made. Calling this
    /// method wihout already connecting the [`Client`] will cause it to hang forever. Solving this
    /// requires running this method in a separate thread, through something like [`tokio::spawn`].
    ///
    /// [`Client`]: crate::Client
    pub async fn publish<T: NetworkTableData>(&self, properties: Properties) -> Result<Publisher<T>, NewPublisherError> {
        Publisher::new(self.name.clone(), properties, self.time.clone(), self.response_timeout, self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    /// Subscribes to this topic with the data type `T`.
    ///
    /// # Note
    /// This method requires the [`Client`] websocket connection to already be made. Calling this
    /// method wihout already connecting the [`Client`] will cause it to hang forever. Solving this
    /// requires running this method in a separate thread, through something like [`tokio::spawn`].
    ///
    /// [`Client`]: crate::Client
    pub async fn subscribe<T: NetworkTableData>(&self, options: SubscriptionOptions) -> Result<Subscriber<T>, NewSubscriberError> {
        Subscriber::new(vec![self.name.clone()], options, self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    // TODO: subscribe to multiple topics
}

