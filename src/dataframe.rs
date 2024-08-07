use std::{collections::HashMap, time::Duration};

use datatype::{DataType, NetworkTableData};
use serde::{de::Visitor, ser::Error, Deserialize, Deserializer, Serialize, Serializer};

// TODO: rename to `type`
pub mod datatype;

// TODO: be able to send multiple messages at once
#[derive(Debug)]
pub enum ServerboundMessage {
    Text(ServerboundTextData),
    Binary(BinaryData),
    Ping,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "lowercase", tag = "method", content = "params")]
pub enum ServerboundTextData {
    Publish(Publish),
    Unpublish(Unpublish),
    SetProperties(SetProperties),

    Subscribe(Subscribe),
    Unsubscribe(Unsubscribe),
}

#[derive(Serialize, Debug)]
pub struct Publish {
    pub name: String,
    pub pubuid: i32,
    pub r#type: DataType,
    pub properties: Properties,
}

#[derive(Serialize, Debug)]
pub struct Unpublish {
    pub pubuid: i32,
}

#[derive(Serialize, Debug)]
pub struct SetProperties {
    pub name: String,
    pub update: Properties,
}

#[derive(Serialize, Debug)]
pub struct Subscribe {
    pub topics: Vec<String>,
    pub subuid: i32,
    pub options: SubscriptionOptions,
}

#[derive(Serialize, Debug)]
pub struct Unsubscribe {
    pub subuid: i32,
}

#[derive(Debug)]
pub enum ClientboundMessage {
    DataFrame(ClientboundDataFrame),
    Pong,
}

#[derive(Debug)]
pub enum ClientboundDataFrame {
    Text(Vec<ClientboundTextData>),
    Binary(Vec<BinaryData>),
}

impl IntoIterator for ClientboundDataFrame {
    type Item = ClientboundData;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        Into::<Vec<ClientboundData>>::into(self).into_iter()
    }
}

#[derive(Debug)]
pub enum ClientboundData {
    Text(ClientboundTextData),
    Binary(BinaryData),
}

impl From<ClientboundDataFrame> for Vec<ClientboundData> {
    fn from(value: ClientboundDataFrame) -> Self {
        match value {
            ClientboundDataFrame::Text(data) => data.into_iter().map(ClientboundData::Text).collect(),
            ClientboundDataFrame::Binary(data) => data.into_iter().map(ClientboundData::Binary).collect(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase", tag = "method", content = "params")]
pub enum ClientboundTextData {
    Announce(Announce),
    Unannounce(Unannounce),
    Properties(PropertiesData),
}

#[derive(Deserialize, Debug)]
pub struct Announce {
    pub name: String,
    pub id: i32,
    pub r#type: DataType,
    pub pubuid: Option<i32>,
    pub properties: Properties,
}

#[derive(Deserialize, Debug)]
pub struct Unannounce {
    pub name: String,
    pub id: i32,
}

#[derive(Deserialize, Debug)]
pub struct PropertiesData {
    pub name: String,
    pub ack: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BinaryData {
    pub id: i32,
    #[serde(serialize_with = "serialize_dur_as_u32", deserialize_with = "deserialize_u32_as_dur")]
    pub timestamp: Duration,
    #[serde(serialize_with = "datatype::serialize_as_u32", deserialize_with = "datatype::deserialize_u32")]
    pub data_type: DataType,
    pub data: rmpv::Value,
}

impl BinaryData {
    pub fn new<T: NetworkTableData>(id: i32, timestamp: Duration, data: T) -> Self {
        Self { id, timestamp, data_type: T::data_type(), data: data.into_value() }
    }
}

/// Topic properties.
///
/// These are properties attached to all topics and are represented as JSON. To add extra
/// properties, use the `extra` field.
///
/// Docs taken and summarized from [here](https://github.com/wpilibsuite/allwpilib/blob/main/ntcore/doc/networktables4.adoc#properties).
#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub struct Properties {
    /// Persistent flag.
    ///
    /// If set to `true`, the server will save this value and it will be restored during server
    /// startup. It will also not be deleted by the server if the last publisher stops publishing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent: Option<bool>,
    /// Retained flag.
    ///
    /// If set to `true`, the server will not delete this topic when the last publisher stops
    /// publishing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained: Option<bool>,
    /// Cached flag.
    ///
    /// If set to `false`, servers and clients will not store the value of this topic meaning only
    /// values updates will be avaible for the topic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,

    /// Extra property values.
    ///
    /// This should be used for generic properties not officially recognized by a `NetworkTables` server.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>
}

/// Options to use when subscribing to a topic.
///
/// To add extra properties, use the `extra` field.
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub struct SubscriptionOptions {
    /// Periodic sweep time in seconds.
    ///
    /// This is how frequently the server should send changes. This value isn't guaranteed by the
    /// server nor the client.
    ///
    /// Default is `100 ms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    // TODO: special (de)serialization to convert to seconds
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
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

fn serialize_dur_as_u32<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer
{
    serializer.serialize_u32(duration.as_micros().try_into().map_err(S::Error::custom)?)
}

fn deserialize_u32_as_dur<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where D: Deserializer<'de>
{
    deserializer.deserialize_u32(DurationVisitor)
}

struct DurationVisitor;

impl<'de> Visitor<'de> for DurationVisitor {
    type Value = Duration;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a valid u32 micros duration")
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where E: serde::de::Error
    {
        self.visit_u64(v.try_into().map_err(E::custom)?)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where E: serde::de::Error
    {
        Ok(Duration::from_micros(v))
    }
}

