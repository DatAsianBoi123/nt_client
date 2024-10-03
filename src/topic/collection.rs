//! Collection of topics that can be used to subscribe to multiple topics at once.

use std::{collections::HashMap, fmt::Debug, sync::Arc};

use tokio::sync::RwLock;

use crate::{data::SubscriptionOptions, subscribe::Subscriber, NTClientSender, NTServerSender, NetworkTablesTime};

use super::{AnnouncedTopic, Topic};

/// Represents a collection of topics.
///
/// This is used to subscribe to multiple topics at once.
///
/// # Examples
/// ```
/// use nt_client::Client;
///
/// # tokio_test::block_on(async {
///     let client = Client::new(Default::default());
///
///     let topics = client.topics(vec![
///         "/topic".to_owned(),
///         "/nested/topic".to_owned(),
///         "/deeply/nested/topic".to_owned(),
///     ]);
///     tokio::spawn(async move {
///         let subscriber = topics.subscribe(Default::default()).await;
///
///         // do something with subscriber...
///     });
///
///     client.connect().await
/// # });
/// ```
#[derive(Clone)]
pub struct TopicCollection {
    names: Vec<String>,
    time: Arc<RwLock<NetworkTablesTime>>,
    announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
    send_ws: NTServerSender,
    recv_ws: NTClientSender,
}

impl IntoIterator for TopicCollection {
    type Item = Topic;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self)
    }
}

impl Debug for TopicCollection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopicCollection")
            .field("topics", &self.names)
            .finish()
    }
}

impl PartialEq for TopicCollection {
    fn eq(&self, other: &Self) -> bool {
        self.names == other.names
    }
}

impl Eq for TopicCollection { }

impl TopicCollection {
    pub(crate) fn new(
        names: Vec<String>,
        time: Arc<RwLock<NetworkTablesTime>>,
        announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
        send_ws: NTServerSender,
        recv_ws: NTClientSender
    ) -> Self {
        Self { names, time, announced_topics, send_ws, recv_ws }
    }

    /// Returns a slice of topic names this collection contains.
    pub fn names(&self) -> &Vec<String> {
        &self.names
    }

    /// Returns a mutable slice of topic names this collection contains.
    pub fn names_mut(&mut self) -> &mut Vec<String> {
        &mut self.names
    }

    /// Subscribes to this collection of topics.
    ///
    /// This method does not require the [`Client`] websocket connection to be made.
    ///
    /// [`Client`]: crate::Client
    pub async fn subscribe(&self, options: SubscriptionOptions) -> Subscriber {
        Subscriber::new(self.names.clone(), options, self.announced_topics.clone(), self.send_ws.clone(), self.recv_ws.subscribe()).await
    }
}

/// Iterator that iterates over [`Topic`]s in a [`TopicCollection`].
///
/// This is obtained by the [`TopicCollection::into_iter`] method.
pub struct IntoIter {
    collection: TopicCollection,
}

impl Iterator for IntoIter {
    type Item = Topic;

    fn next(&mut self) -> Option<Self::Item> {
        let collection = &mut self.collection;
        if collection.names.is_empty() { return None; };

        Some(Topic::new(
                collection.names.remove(0),
                collection.time.clone(),
                collection.announced_topics.clone(),
                collection.send_ws.clone(),
                collection.recv_ws.clone(),
        ))
    }
}

impl IntoIter {
    pub(self) fn new(collection: TopicCollection) -> Self {
        IntoIter {
            collection,
        }
    }
}

