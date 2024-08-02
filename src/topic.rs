use std::sync::Arc;

use tokio::sync::RwLock;

use crate::{dataframe::{datatype::NetworkTableDataType, Properties, SubscriptionOptions}, publish::{NewPublisherError, Publisher}, subscribe::{NewSubscriberError, Subscriber}, NTClientSender, NTServerSender, NetworkTablesTime};

/// Represents a `NetworkTables` topic.
///
/// The intended method to obtain one of these is to use the [`Client::topic`] method.
///
/// [`Client::topic`]: crate::Client::topic
#[derive(Clone)]
pub struct Topic {
    name: String,
    time: Arc<RwLock<NetworkTablesTime>>,
    send_ws: NTServerSender,
    recv_ws: NTClientSender,
}

impl Topic {
    // NOTE: pub(super) or just pub?
    pub(super) fn new(name: String, time: Arc<RwLock<NetworkTablesTime>>, send_ws: NTServerSender, recv_ws: NTClientSender) -> Self {
        Self { name, time, send_ws, recv_ws }
    }

    /// Publishes to this topic.
    ///
    /// # Note
    /// This method requires the [`Client`] websocket connection to already be made. Calling this
    /// method wihout already connecting the [`Client`] will cause it to hang forever. Solving this
    /// requires running this method in a separate thread, through something like [`tokio::spawn`].
    ///
    /// [`Client`]: crate::Client
    pub async fn publish<T: NetworkTableDataType>(&self, properties: Properties) -> Result<Publisher<T>, NewPublisherError> {
        Publisher::new(self.name.clone(), properties, self.time.clone(), self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    /// Subscribes to this topic.
    ///
    /// # Note
    /// This method requires the [`Client`] websocket connection to already be made. Calling this
    /// method wihout already connecting the [`Client`] will cause it to hang forever. Solving this
    /// requires running this method in a separate thread, through something like [`tokio::spawn`].
    ///
    /// [`Client`]: crate::Client
    pub async fn subscribe<T: NetworkTableDataType>(&self, options: SubscriptionOptions) -> Result<Subscriber<T>, NewSubscriberError> {
        Subscriber::new(vec![self.name.clone()], options, self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    // TODO: subscribe to multiple topics
}

