//! Named data channels.
//!
//! Topics have a fixed data type and can be subscribed and published to.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::sync::{broadcast, RwLock};

use crate::{data::{r#type::{DataType, NetworkTableData}, Announce, Properties, SubscriptionOptions}, publish::{NewPublisherError, Publisher}, subscribe::Subscriber, NTClientSender, NTServerSender, NetworkTablesTime};

/// Represents a `NetworkTables` topic.
///
/// This differs from an [`AnnouncedTopic`], as that is a **server created topic**, while this is a
/// **client created topic**.
///
/// The intended method to obtain one of these is to use the [`Client::topic`] method.
///
/// [`Client::topic`]: crate::Client::topic
#[derive(Debug, Clone)]
pub struct Topic {
    name: String,
    time: Arc<RwLock<NetworkTablesTime>>,
    announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
    response_timeout: Duration,
    send_ws: NTServerSender,
    recv_ws: NTClientSender,
}

impl PartialEq for Topic {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Topic { }

impl Topic {
    // NOTE: pub(super) or just pub?
    pub(super) fn new(
        name: String,
        time: Arc<RwLock<NetworkTablesTime>>,
        announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
        response_timeout: Duration,
        send_ws: NTServerSender,
        recv_ws: NTClientSender,
    ) -> Self {
        Self { name, time, announced_topics, response_timeout, send_ws, recv_ws }
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

    /// Subscribes to this topic.
    ///
    /// This method does not require the [`Client`] websocket connection to be made.
    ///
    /// [`Client`]: crate::Client
    pub async fn subscribe(&self, options: SubscriptionOptions) -> Result<Subscriber, broadcast::error::RecvError> {
        Subscriber::new(vec![self.name.clone()], options, self.announced_topics.clone(), self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    // TODO: subscribe to multiple topics
}

/// A topic that has been announced by the `NetworkTables` server.
///
/// Topics will only be announced when there is a subscriber subscribing to it.
///
/// This differs from a [`Topic`], as that is a **client created topic**, while this is a
/// **server created topic**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncedTopic {
    name: String,
    id: i32,
    r#type: DataType,
    properties: Properties,
}

impl AnnouncedTopic {
    /// Gets the name of this topic.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gets the id of this topic.
    ///
    /// This id is guaranteed to be unique.
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Gets the data type of this topic.
    pub fn r#type(&self) -> &DataType {
        &self.r#type
    }

    /// Gets the properties of this topic.
    pub fn properties(&self) -> &Properties {
        &self.properties
    }

    /// Returns whether the given topic names and subscription options match this topic.
    pub fn matches(&self, names: &[String], options: &SubscriptionOptions) -> bool {
        names.iter()
            .any(|name| &self.name == name || (options.prefix.is_some_and(|flag| flag) && self.name.starts_with(name)))
    }
}

impl From<&Announce> for AnnouncedTopic {
    fn from(value: &Announce) -> Self {
        Self {
            name: value.name.clone(),
            id: value.id,
            r#type: value.r#type.clone(),
            properties: value.properties.clone(),
        }
    }
}

