//! Topic subscribers.
//!
//! Subscribers receive data value updates to a topic.
//!
//! # Examples
//!
//! ```no_run
//! use nt_client::{subscribe::ReceivedMessage, data::NetworkTableData, Client};
//!
//! # tokio_test::block_on(async {
//! let client = Client::new(Default::default());
//!
//! client.connect_setup(setup).await.unwrap();
//! # });
//!
//! fn setup(client: &Client) {
//!     // prints updates to the `/counter` topic to the stdout
//!     let counter_topic = client.topic("/counter");
//!     tokio::spawn(async move {
//!         // subscribes to the `/counter`
//!         let mut subscriber = counter_topic.subscribe(Default::default()).await.unwrap();
//!
//!         loop {
//!             match subscriber.recv().await {
//!                 Ok(ReceivedMessage::Updated((_topic, value))) => {
//!                     // get the updated value as an `i32`
//!                     let number = i32::from_value(value).unwrap();
//!                     println!("counter updated to {number}");
//!                 },
//!                 Ok(ReceivedMessage::Announced(topic)) => println!("announced topic: {topic:?}"),
//!                 Ok(ReceivedMessage::Unannounced { name, .. }) => println!("unannounced topic: {name}"),
//!                 Ok(ReceivedMessage::UpdateProperties(topic)) => println!("topic {} had its properties updated: {:?}", topic.name(), topic.properties()),
//!                 Err(err) => {
//!                     eprintln!("got error: {err:?}");
//!                     break;
//!                 },
//!             }
//!         }
//!     });
//! }

use std::{collections::{HashMap, HashSet}, fmt::Debug, sync::Arc, time::Duration};

use futures_util::future::join_all;
use rmpv::Utf8String;
use serde::{Serialize, Serializer};
use tokio::sync::{broadcast, RwLock};
use tracing::warn;

use crate::{NTClientReceiver, NTServerSender, error::ConnectionClosedError, net::{BinaryData, ClientboundData, ClientboundTextData, PropertiesData, ServerboundMessage, ServerboundTextData, Subscribe, Unsubscribe}, recv_until_async, topic::{AnnouncedTopic, AnnouncedTopics}};

/// A `NetworkTables` subscriber that subscribes to a [`Topic`].
///
/// Subscribers receive topic announcements, value updates, topic unannouncements, and topic
/// property change messages.
///
/// This will automatically get unsubscribed whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Subscriber {
    topics: Vec<String>,
    id: i32,
    options: SubscriptionOptions,
    topic_ids: Arc<RwLock<HashSet<i32>>>,
    announced_topics: Arc<RwLock<AnnouncedTopics>>,

    ws_sender: NTServerSender,
    ws_recv: NTClientReceiver,
}

impl Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscriber")
            .field("topics", &self.topics)
            .field("id", &self.id)
            .field("options", &self.options)
            .field("topic_ids", &self.topic_ids)
            .finish()
    }
}

impl PartialEq for Subscriber {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Subscriber { }

impl Subscriber {
    pub(super) async fn new(
        topics: Vec<String>,
        options: SubscriptionOptions,
        announced_topics: Arc<RwLock<AnnouncedTopics>>,
        ws_sender: NTServerSender,
        ws_recv: NTClientReceiver,
    ) -> Result<Self, ConnectionClosedError> {
        let id = rand::random();

        let topic_ids = {
            let announced_topics = announced_topics.read().await;
            announced_topics.id_values()
                .filter(|topic| topic.matches(&topics, &options))
                .map(|topic| topic.id())
                .collect()
        };

        let sub_message = ServerboundTextData::Subscribe(Subscribe { topics: topics.clone(), subuid: id, options: options.clone() });
        ws_sender.send(ServerboundMessage::Text(sub_message)).map_err(|_| ConnectionClosedError)?;

        Ok(Self {
            topics,
            id,
            options,
            topic_ids: Arc::new(RwLock::new(topic_ids)),
            announced_topics,
            ws_sender,
            ws_recv
        })
    }

    /// Unsubscribes from the topic(s).
    ///
    /// This behaves exactly the same as [std::mem::drop]ping this subscriber, since the destructor
    /// for [`Subscriber`]s automatically unsubscribes.
    pub fn unsubscribe(self) { }

    /// Returns all topics that this subscriber is subscribed to.
    pub async fn topics(&self) -> HashMap<i32, AnnouncedTopic> {
        let topic_ids = self.topic_ids.clone();
        let topic_ids = topic_ids.read().await;
        let mapped_futures = topic_ids.iter()
            .map(|id| {
                let announced_topics = self.announced_topics.clone();
                async move {
                    (*id, announced_topics.read().await.get_from_id(*id).expect("topic exists").clone())
                }
            });
        join_all(mapped_futures).await.into_iter().collect()
    }

    /// Receives the next value for this subscriber.
    ///
    /// Topics that have already been announced will not be received by this method. To view
    /// all topics that are being subscribed to, use the [`topics`][`Self::topics`] method.
    ///
    /// # Errors
    /// Returns an error if something goes wrong when receiving messages from the client.
    pub async fn recv(&mut self) -> Result<ReceivedMessage, broadcast::error::RecvError> {
        recv_until_async(&mut self.ws_recv, |data| {
            let topic_ids = self.topic_ids.clone();
            let announced_topics = self.announced_topics.clone();
            let topics = &self.topics;
            let options = &self.options;
            async move {
                match *data {
                    ClientboundData::Binary(BinaryData { id, ref timestamp, ref data, .. }) => {
                        let contains = {
                            topic_ids.read().await.contains(&id)
                        };
                        if !contains { return None; };
                        let announced_topic = {
                            let mut topics = announced_topics.write().await;
                            let Some(topic) = topics.get_mut_from_id(id) else {
                                warn!("unknown topic with id {id}");
                                return None;
                            };

                            if topic.last_updated().is_some_and(|last_timestamp| last_timestamp > timestamp) { return None; };
                            topic.update(*timestamp);
                            if topic.properties().cached.is_some_and(|cached| cached) {
                                topic.update_value(data.clone());
                            };

                            topic.clone()
                        };
                        Some(ReceivedMessage::Updated((announced_topic, data.clone())))
                    },
                    ClientboundData::Text(ClientboundTextData::Announce(ref announce)) => {
                        let matches = announced_topics.read().await.get_from_id(announce.id).is_some_and(|topic| topic.matches(topics, options));
                        if matches {
                            topic_ids.write().await.insert(announce.id);
                            Some(ReceivedMessage::Announced(announce.into()))
                        } else { None }
                    },
                    ClientboundData::Text(ClientboundTextData::Unannounce(ref unannounce)) => {
                        topic_ids.write().await.remove(&unannounce.id).then(|| {
                            ReceivedMessage::Unannounced { name: unannounce.name.clone(), id: unannounce.id }
                        })
                    },
                    ClientboundData::Text(ClientboundTextData::Properties(PropertiesData { ref name, .. })) => {
                        let (contains, id) = {
                            let Some(id) = announced_topics.read().await.get_id(name) else {
                                warn!("unknown topic {name}");
                                return None;
                            };
                            (topic_ids.read().await.contains(&id), id)
                        };
                        if !contains { return None; };

                        let topics = announced_topics.read().await;
                        let topic = topics.get_from_id(id).expect("topic exists").clone();
                        Some(ReceivedMessage::UpdateProperties(topic))
                    },
                }
            }
        }).await
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        let unsub_message = ServerboundTextData::Unsubscribe(Unsubscribe { subuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(unsub_message));
    }
}

/// Options to use when subscribing to a topic.
///
/// To add extra properties, use the `extra` field.
#[derive(Serialize, Default, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub struct SubscriptionOptions {
    /// Periodic sweep time in seconds.
    ///
    /// This is how frequently the server should send changes. This value isn't guaranteed by the
    /// server nor the client.
    ///
    /// Default is `100 ms`.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serialize_dur_as_secs")]
    pub periodic: Option<Duration>,
    /// All changes flag.
    ///
    /// If `true`, all value changes are sent when subscribing rather than just the most recent
    /// value.
    ///
    /// Default is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
    /// No value changes flag.
    ///
    /// If `true`, the client will only receive topic announce messages and not value changes.
    ///
    /// Default is `false`.
    #[serde(rename = "topicsonly", skip_serializing_if = "Option::is_none")]
    pub topics_only: Option<bool>,
    /// Prefix flag.
    ///
    /// If `true`, all topics starting with the name of the topic(s) will be subscribed to.
    ///
    /// Default is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<bool>,

    /// Extra data.
    ///
    /// This should be used for generic options not officially recognized by a `NetworkTables` server.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl SubscriptionOptions {
    /// Converts from a `msgpack` map.
    pub fn from_msgpack_map(map: Vec<(rmpv::Value, rmpv::Value)>) -> Option<Self> {
        let mut periodic = None;
        let mut all = None;
        let mut topics_only = None;
        let mut prefix = None;
        let mut extra = HashMap::new();

        for (key, value) in map {
            let key = if let rmpv::Value::String(key) = key {
                if let Some(key) = key.into_str() {
                    key
                } else {
                    warn!("key has non-utf8 characters");
                    continue;
                }
            } else {
                warn!("non-string key for subscription option {key:?}");
                continue;
            };
            match (key.as_ref(), value) {
                ("periodic", rmpv::Value::Integer(int)) if int.is_u64() => periodic = int.as_u64().map(Duration::from_secs),
                ("all", rmpv::Value::Boolean(bool)) => all = Some(bool),
                ("topicsonly", rmpv::Value::Boolean(bool)) => topics_only = Some(bool),
                ("prefix", rmpv::Value::Boolean(bool)) => prefix = Some(bool),
                (_, value) => {
                    extra.insert(key, Self::value_from_msgpack(value)?);
                }
            }
        }

        Some(Self {
            periodic,
            all,
            topics_only,
            prefix,
            extra,
        })
    }

    /// Converts these options into a `msgpack` map.
    pub fn into_msgpack_map(self) -> Vec<(rmpv::Value, rmpv::Value)> {
        let mut map = Vec::new();
        if let Some(periodic) = self.periodic {
            map.push((rmpv::Value::String("periodic".into()), rmpv::Value::Integer(periodic.as_secs().into())));
        };
        if let Some(all) = self.all {
            map.push((rmpv::Value::String("all".into()), rmpv::Value::Boolean(all)));
        };
        if let Some(topics_only) = self.topics_only {
            map.push((rmpv::Value::String("topicsonly".into()), rmpv::Value::Boolean(topics_only)));
        };
        if let Some(prefix) = self.prefix {
            map.push((rmpv::Value::String("prefix".into()), rmpv::Value::Boolean(prefix)));
        };
        for (key, value) in self.extra {
            map.push((rmpv::Value::String(Utf8String::from(key)), Self::value_into_msgpack(value)));
        }
        map
    }

    fn value_from_msgpack(value: rmpv::Value) -> Option<serde_json::Value> {
        match value {
            rmpv::Value::Nil => Some(serde_json::Value::Null),
            rmpv::Value::Integer(int) if int.is_u64() => {
                Some(serde_json::Value::Number(serde_json::Number::from_u128(int.as_u64().expect("int is a u64") as u128).expect("number fits within a u64")))
            },
            rmpv::Value::Integer(int) /* if int.is_i64() */ => {
                Some(serde_json::Value::Number(serde_json::Number::from_i128(int.as_i64().expect("int is a i64") as i128).expect("number fits within a i64")))
            },
            rmpv::Value::F64(f64) => Some(serde_json::Value::Number(serde_json::Number::from_f64(f64)?)),
            rmpv::Value::F32(f32) => Some(serde_json::Value::Number(serde_json::Number::from_f64(f32 as f64)?)),
            rmpv::Value::Boolean(bool) => Some(serde_json::Value::Bool(bool)),
            rmpv::Value::String(str) => Some(serde_json::Value::String(str.into_str()?)),
            rmpv::Value::Array(array) => Some(serde_json::Value::Array(array.into_iter()
                .map(Self::value_from_msgpack)
                .collect::<Option<_>>()?)),
            rmpv::Value::Map(map) => Some(serde_json::Value::Object(map.into_iter()
                .map(|(key, value)| {
                    let key = if let rmpv::Value::String(key) = key {
                        key.into_str()?
                    } else {
                        return None;
                    };
                    Self::value_from_msgpack(value).map(|value| (key, value))
                })
                .collect::<Option<_>>()?)),
            _ => None,
        }
    }

    fn value_into_msgpack(value: serde_json::Value) -> rmpv::Value {
        match value {
            serde_json::Value::Null => rmpv::Value::Nil,
            serde_json::Value::Bool(bool) => rmpv::Value::Boolean(bool),
            serde_json::Value::Number(number) if number.is_i64() => rmpv::Value::Integer(rmpv::Integer::from(number.as_i64().expect("number is i64"))),
            serde_json::Value::Number(number) if number.is_u64() => rmpv::Value::Integer(rmpv::Integer::from(number.as_u64().expect("number is u64"))),
            serde_json::Value::Number(number) /* if number.is_f64() */ => rmpv::Value::F64(number.as_f64().expect("number is f64")),
            serde_json::Value::String(string) => rmpv::Value::String(Utf8String::from(string)),
            serde_json::Value::Array(values) => rmpv::Value::Array(values.into_iter()
                .map(Self::value_into_msgpack)
                .collect()),
            serde_json::Value::Object(map) => rmpv::Value::Map(map.into_iter()
                .map(|(key, value)| (rmpv::Value::String(Utf8String::from(key)), Self::value_into_msgpack(value)))
                .collect()),
        }
    }
}

fn serialize_dur_as_secs<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer
{
    if let Some(duration) = duration {
        serializer.serialize_f64(duration.as_secs_f64())
    } else {
        serializer.serialize_none()
    }
}

/// Messages that can received from a subscriber.
#[derive(Debug, Clone, PartialEq)]
pub enum ReceivedMessage {
    /// A topic that matches the subscription options and subscribed topics was announced.
    ///
    /// This will always be received before any updates for that topic are sent.
    Announced(AnnouncedTopic),
    /// An subscribed topic was updated.
    ///
    /// Subscribed topics are any topics that were [`Announced`][`ReceivedMessage::Announced`].
    /// Only the most recent updated value is sent.
    Updated((AnnouncedTopic, rmpv::Value)),
    /// An announced topic had its properties updated.
    UpdateProperties(AnnouncedTopic),
    /// An announced topic was unannounced.
    Unannounced {
        /// The name of the topic that was unannounced.
        name: String,
        /// The id of the topic that was unannounced.
        id: i32,
    },
}

