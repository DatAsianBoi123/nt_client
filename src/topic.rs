//! Named data channels.
//!
//! Topics have a fixed data type and can be subscribed and published to.

use std::{collections::{HashMap, VecDeque}, fmt::Display, ops::{Deref, DerefMut}, sync::Arc};

use tokio::sync::RwLock;

use crate::{data::{r#type::{DataType, NetworkTableData}, Announce, Properties, SubscriptionOptions}, publish::{NewPublisherError, Publisher}, subscribe::Subscriber, NTClientSender, NTServerSender, NetworkTablesTime};

#[macro_export]
/// Creates a [`TopicPath`] containing the segments.
///
/// `path!` allows for the easy creation of [`TopicPath`]s without having to deal with creating
/// [`VecDeque`]s.
/// Its usage is extremely similar to [`std::vec!`].
///
/// # Examples
/// ```
/// use std::collections::VecDeque;
/// use nt_client::{topic::TopicPath, path};
///
/// let mut vec_deque = VecDeque::new();
/// vec_deque.push_back("my".to_owned());
/// vec_deque.push_back("path".to_owned());
/// let path = TopicPath::new(vec_deque);
/// 
/// assert_eq!(path!["my", "path"], path);
/// ```
macro_rules! path {
    () => {
        $crate::topic::TopicPath::default();
    };

    ($($segment: literal),+ $(,)?) => {{
        let mut segments = std::collections::VecDeque::new();
        $(
            segments.push_back($segment.to_string());
        )*
        $crate::topic::TopicPath::new(segments)
    }};
}

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
    pub(super) fn new(
        name: String,
        time: Arc<RwLock<NetworkTablesTime>>,
        announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
        send_ws: NTServerSender,
        recv_ws: NTClientSender,
    ) -> Self {
        Self { name, time, announced_topics, send_ws, recv_ws }
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
        Publisher::new(self.name.clone(), properties, self.time.clone(), self.send_ws.clone(), self.recv_ws.subscribe()).await
    }

    /// Subscribes to this topic.
    ///
    /// This method does not require the [`Client`] websocket connection to be made.
    ///
    /// [`Client`]: crate::Client
    pub async fn subscribe(&self, options: SubscriptionOptions) -> Subscriber {
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
    /// Returns the name of this topic.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the id of this topic.
    ///
    /// This id is guaranteed to be unique.
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Returns the data type of this topic.
    pub fn r#type(&self) -> &DataType {
        &self.r#type
    }

    /// Returns the properties of this topic.
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

/// Represents a slash (`/`) deliminated path.
///
/// This is especially useful when trying to parse nested data, such as from Shuffleboard
/// (found in `/Shuffleboard/...`).
///
/// This can be thought of as a wrapper for a [`VecDeque`], only providing trait impls to convert
/// to/from a [`String`].
///
/// # Note
/// The [`Display`] impl will always contain a leading slash, but not a trailing one,
/// regardless of if the path was parsed from a [`String`] containing either a leading or trailing
/// slash.
///
/// # Warning
/// In cases where slashes are present in segment names, turning to and from a [`String`] is
/// **NOT** guaranteed to preserve segment names.
///
/// ```should_panic
/// use nt_client::{topic::TopicPath, path};
///
/// let path = path!["///weird//", "na//mes//"];
///
/// // this will panic!
/// assert_eq!(<String as Into<TopicPath>>::into(path.to_string()), path);
/// ```
///
/// In the above example, `.to_string()` is converting the path to `////weird///na//mes//`.
/// When turning this back into a `TopicPath`, it recognizes the following segments (with
/// trailing and leading slashes removed):
///
/// **/** / **/weird** / **/** / **na** / **/mes** /
///
/// # Examples
/// ```
/// use nt_client::{topic::TopicPath, path};
///
/// // automatically inserts a leading slash
/// assert_eq!(path!["my", "topic"].to_string(), "/my/topic");
///
/// // slashes in the segment names are preserved
/// assert_eq!(path!["some", "/data"].to_string(), "/some//data");
///
/// assert_eq!(<&str as Into<TopicPath>>::into("/path/to/data"), path!["path", "to", "data"]);
///
/// assert_eq!(<&str as Into<TopicPath>>::into("//some///weird/path/"), path!["/some", "/", "weird", "path"]);
/// ```
/// Getting a topic:
/// ```no_run
/// use nt_client::{Client, path};
///
/// # tokio_test::block_on(async {
/// let client = Client::new(Default::default());
///
/// let topic = client.topic(path!["nested", "data"]);
///
/// // do something with `topic`
///
/// client.connect().await;
/// # });
/// ```
/// Parsing topic name:
/// ```no_run
/// use nt_client::{topic::TopicPath, data::SubscriptionOptions, subscribe::ReceivedMessage, Client};
///
/// # tokio_test::block_on(async {
/// let client = Client::new(Default::default());
///
/// let sub_topic = client.topic("/Root/");
/// tokio::spawn(async move {
///     let mut sub = sub_topic.subscribe(SubscriptionOptions {
///         topics_only: Some(true),
///         prefix: Some(true),
///         ..Default::default()
///     }).await;
///
///     while let Ok(ReceivedMessage::Announced(topic)) = sub.recv().await {
///         let path: TopicPath = topic.name().into();
///
///         // do something with `path`
///     }
/// });
///
/// client.connect().await;
///
/// # });
/// ```
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicPath {
    segments: VecDeque<String>,
}

impl TopicPath {
    /// The delimiter to use when converting from a [`String`].
    pub const DELIMITER: char = '/';

    /// Creates a new `TopicPath` with segments.
    pub fn new(segments: VecDeque<String>) -> Self {
        Self { segments }
    }
}

impl From<VecDeque<String>> for TopicPath {
    fn from(value: VecDeque<String>) -> Self {
        Self { segments: value }
    }
}

impl Deref for TopicPath {
    type Target = VecDeque<String>;

    fn deref(&self) -> &Self::Target {
        &self.segments
    }
}

impl DerefMut for TopicPath {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.segments
    }
}

impl Display for TopicPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let full_path = self.segments.iter().fold(String::new(), |prev, curr| prev + "/" + curr);
        f.write_str(&full_path)
    }
}

impl From<&str> for TopicPath {
    fn from(value: &str) -> Self {
        value.to_string().into()
    }
}

impl From<String> for TopicPath {
    fn from(value: String) -> Self {
        let str = value.strip_prefix(Self::DELIMITER).unwrap_or(&value);
        let str = str
            .strip_suffix(Self::DELIMITER)
            .map(|str| str.to_owned())
            .unwrap_or_else(|| str.to_owned());

        str.chars().fold((VecDeque::<String>::new(), true), |(mut parts, prev_is_delimiter), char| {
            if prev_is_delimiter {
                parts.push_back(String::from(char));
                (parts, false)
            } else {
                let is_delimiter = char == Self::DELIMITER;
                if !is_delimiter { parts.back_mut().unwrap().push(char); };
                (parts, is_delimiter)
            }
        }).0.into()
    }
}

