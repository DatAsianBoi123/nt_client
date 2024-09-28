//! Publisher portion of the `NetworkTables` spec.
//!
//! Publishers are used to set new values for topics that can be seen by subscribers.
//!
//! # Examples
//!
//! ```no_run
//! use std::time::Duration;
//! use nt_client::Client;
//!
//! # tokio_test::block_on(async {
//! let client = Client::new(Default::default());
//!
//! // increments the `/counter` topic every 5 seconds
//! let counter_topic = client.topic("/counter");
//! tokio::spawn(async move {
//!     const INCREMENT_INTERVAL: Duration = Duration::from_secs(5);
//!     
//!     let mut publisher = counter_topic.publish::<u32>(Default::default()).await.unwrap();
//!     let mut interval = tokio::time::interval(INCREMENT_INTERVAL);
//!     let mut counter = 0;
//!     
//!     loop {
//!         interval.tick().await;
//!
//!         publisher.set(counter).await.expect("connection is still alive");
//!         counter += 1;
//!     }
//! });
//!
//! client.connect().await.unwrap();
//! # });

use std::{collections::HashMap, fmt::Debug, marker::PhantomData, sync::Arc, time::Duration};

use serde::Serialize;
use tokio::sync::{broadcast, RwLock};
use tracing::debug;

use crate::{data::{r#type::{DataType, NetworkTableData}, Announce, BinaryData, ClientboundData, ClientboundTextData, Properties, PropertiesData, Publish, ServerboundMessage, ServerboundTextData, SetProperties, Unpublish}, error::ConnectionClosedError, recv_until, NTClientReceiver, NTServerSender, NetworkTablesTime};

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Publisher<T: NetworkTableData> {
    _phantom: PhantomData<T>,
    topic: String,
    id: i32,
    time: Arc<RwLock<NetworkTablesTime>>,
    ws_sender: NTServerSender,
    ws_recv: NTClientReceiver,
}

impl<T: NetworkTableData> Debug for Publisher<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher")
            .field("id", &self.id)
            .field("type", &T::data_type())
            .finish()
    }
}

impl<T: NetworkTableData> PartialEq for Publisher<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T: NetworkTableData> Eq for Publisher<T> { }

impl<T: NetworkTableData> Publisher<T> {
    pub(super) async fn new(
        name: String,
        properties: Properties,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        mut ws_recv: NTClientReceiver,
    ) -> Result<Self, NewPublisherError> {
        let id = rand::random();
        let pub_message = ServerboundTextData::Publish(Publish { name, pubuid: id, r#type: T::data_type(), properties });
        ws_sender.send(ServerboundMessage::Text(pub_message).into()).map_err(|_| broadcast::error::RecvError::Closed)?;

        let (name, r#type, id) = {
            recv_until(&mut ws_recv, |data| {
                if let ClientboundData::Text(ClientboundTextData::Announce(Announce { ref name, ref r#type, pubuid: Some(pubuid), .. })) = *data {
                    // TODO: cached property

                    Some((name.clone(), r#type.clone(), pubuid))
                } else {
                    None
                }
            }).await
        }?;
        if T::data_type() != r#type { return Err(NewPublisherError::MismatchedType { server: r#type, client: T::data_type() }); };

        debug!("[pub {id}] publishing to topic `{name}`");
        Ok(Self { _phantom: PhantomData, topic: name, id, time, ws_sender, ws_recv })
    }

    /// Publish a new value to the [`Topic`].
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set(&self, value: T) -> Result<(), ConnectionClosedError> {
        let time = self.time.read().await;
        self.set_time(value, time.server_time()).await
    }

    /// Publishes a default value to the [`Topic`].
    ///
    /// This default value will only be seen by other clients and the server if no other value has
    /// been published to the [`Topic`] yet.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set_default(&self, value: T) -> Result<(), ConnectionClosedError> {
        self.set_time(value, Duration::ZERO).await
    }

    /// Updates the properties of the topic being subscribed to, returning a `future` that
    /// completes when the server acknowledges the update.
    ///
    /// A [`SetPropsBuilder`] should be used for easy creation of updated properties.
    ///
    /// # Errors
    /// Returns an error if messages could not be received from the `NetworkTables` server.
    ///
    /// # Examples
    /// ```no_run
    /// use std::time::Duration;
    /// use nt_client::{publish::SetPropsBuilder, Client};
    ///
    /// # tokio_test::block_on(async {
    /// let client = Client::new(Default::default());
    ///
    /// let topic = client.topic("mytopic");
    /// tokio::spawn(async move {
    ///     let mut sub = topic.publish::<String>(Default::default()).await.unwrap();
    ///
    ///     // update properties after 5 seconds
    ///     tokio::time::sleep(Duration::from_secs(5)).await;
    ///
    ///     // Props:
    ///     // - set `retained` to true
    ///     // - delete `arbitrary property`
    ///     // everything else stays unchanged
    ///     let props = SetPropsBuilder::new()
    ///         .replace_retained(true)
    ///         .delete("arbitrary property".to_owned())
    ///         .build();
    ///
    ///     sub.update_props(props).await.unwrap();
    /// });
    ///
    /// client.connect().await.unwrap();
    /// # })
    /// ```
    // TODO: probably replace with custom error
    pub async fn update_props(&mut self, new_props: HashMap<String, Option<String>>) -> Result<(), broadcast::error::RecvError> {
        self.ws_sender.send(ServerboundMessage::Text(ServerboundTextData::SetProperties(SetProperties {
            name: self.topic.clone(),
            update: new_props,
        })).into()).map_err(|_| broadcast::error::RecvError::Closed)?;

        recv_until(&mut self.ws_recv, |data| {
            if let ClientboundData::Text(ClientboundTextData::Properties(PropertiesData { ref name, ack })) = *data {
                // TODO: create and return Properties
                if ack.is_some_and(|ack| ack) && name == &self.topic { Some(()) }
                else { None }
            } else {
                None
            }
        }).await?;

        Ok(())
    }

    async fn set_time(&self, data: T, timestamp: Duration) -> Result<(), ConnectionClosedError> {
        let data_value = data.clone().into_value();
        let binary = BinaryData::new(self.id, timestamp, data);
        self.ws_sender.send(ServerboundMessage::Binary(binary).into()).map_err(|_| ConnectionClosedError)?;
        debug!("[pub {}] set to {data_value} at time {timestamp:?}", self.id);
        Ok(())
    }
}

impl<T: NetworkTableData> Drop for Publisher<T> {
    fn drop(&mut self) {
        let data = ServerboundTextData::Unpublish(Unpublish { pubuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(data).into());
        debug!("[pub {}] unpublished", self.id);
    }
}

/// Errors that can occur when creating a new [`Publisher`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum NewPublisherError {
    /// An error occurred when receiving data from the connection.
    #[error(transparent)]
    // TODO: probably replace with custom error
    Recv(#[from] broadcast::error::RecvError),
    /// The server and client have mismatched data types.
    ///
    /// This can occur if, for example, the client is publishing [`String`]s to a topic that the
    /// server has a different data type for, like an [`i32`].
    #[error("mismatched data types! server has {server:?}, but tried to use {client:?} instead")]
    MismatchedType {
        /// The server's data type.
        server: DataType,
        /// The client's data type.
        client: DataType,
    },
}

macro_rules! builder {
    ($lit: literal : [
        $( #[$s_m: meta ] )* fn $set: ident,
        $( #[ $r_m: meta ] )* fn $replace: ident,
        $( #[$d_m: meta ] )* fn $delete: ident,
        $( #[ $u_m: meta ] )* fn $unchange: ident,
    ] : $ty: ty) => {
        $( #[ $s_m ] )*
        pub fn $set(self, value: Option<$ty>) -> Self {
            self.set($lit.to_owned(), value.map(|value| value.to_string()))
        }
        $( #[ $r_m ] )*
        pub fn $replace(self, value: $ty) -> Self {
            self.replace($lit.to_owned(), value.to_string())
        }
        $( #[ $d_m ] )*
        pub fn $delete(self) -> Self {
            self.delete($lit.to_owned())
        }
        $( #[ $u_m ] )*
        pub fn $unchange(self) -> Self {
            self.unchange($lit.to_owned())
        }
    };
}

/// Convenient builder used when updating topic properties.
#[derive(Serialize, Default, Debug, Clone, PartialEq, Eq)]
pub struct SetPropsBuilder {
    inner: HashMap<String, Option<String>>,
}

impl SetPropsBuilder {
    /// Creates a new, empty builder.
    ///
    /// This is identical to calling [`Default::default`].
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a new builder updating certain server-recognized properties.
    ///
    /// This method differs from [`with_props_unchange`][`Self::with_props_unchange`] because
    /// unlike that method, this method deletes the property if it's [`None`].
    ///
    /// With the `extra` field, if the key is not present in the map, it does not get updated. If
    /// the key is present, it gets set to the value.
    ///
    /// Behavior with the `extra` field is identical in both methods.
    ///
    /// # Examples
    /// ```
    /// use nt_client::{data::Properties, publish::SetPropsBuilder};
    ///
    /// // properties are:
    /// // - persistent: `true`
    /// // - cached: `true`
    /// // - retained: unset (defaults to `false`)
    /// let properties = Properties { persistent: Some(true), cached: Some(true), ..Default::default() };
    ///
    /// // update properties is:
    /// // - set `persistent` to `true`
    /// // - set `cached` to `true`
    /// // - delete `retained`
    /// // everything else stays unchanged
    /// let builder = SetPropsBuilder::with_props_delete(properties);
    /// ```
    pub fn with_props_delete(Properties { persistent, retained, cached, extra }: Properties) -> Self {
        let mut builder = Self::new()
            .set_persistent(persistent)
            .set_retained(retained)
            .set_cached(cached);

        for (key, value) in extra {
            builder = builder.replace(key, value);
        }

        builder
    }

    /// Creates a new builder updating certain server-recognized properties.
    ///
    /// This method differs from [`with_props_delete`][`Self::with_props_delete`] because
    /// unlike that method, this method does not delete the property if it's [`None`]. Rather, it
    /// keeps it unchanged.
    ///
    /// With the `extra` field, if the key is not present in the map, it does not get updated. If
    /// the key is present, it gets set to the value.
    ///
    /// Behavior with the `extra` field is identical in both methods.
    ///
    /// # Examples
    /// ```
    /// use nt_client::{data::Properties, publish::SetPropsBuilder};
    ///
    /// // properties are:
    /// // - persistent: `true`
    /// // - cached: `true`
    /// // - retained: unset (defaults to `false`)
    /// let properties = Properties { persistent: Some(true), cached: Some(true), ..Default::default() };
    ///
    /// // update properties is:
    /// // - set `persistent` to `true`
    /// // - set `cached` to `true`
    /// // everything else stays unchanged
    /// let builder = SetPropsBuilder::with_props_delete(properties);
    /// ```
    pub fn with_props_unchange(Properties { persistent, retained, cached, extra }: Properties) -> Self {
        macro_rules! replace_or_unchange {
            ($map: ident += ($name: literal , $val: expr)) => {
                if let Some(val) = $val { $map.insert($name.to_string(), Some(val.to_string())); };
            };
        }

        let mut map = HashMap::new();
        replace_or_unchange!(map += ("persistent", persistent));
        replace_or_unchange!(map += ("retained", retained));
        replace_or_unchange!(map += ("cached", cached));

        map.extend(extra.into_iter().map(|(key, value)| (key, Some(value))));

        Self { inner: map }
    }

    /// Sets a key to a value.
    ///
    /// A value of [`None`] indicates that the key should be deleted on the server's end.
    pub fn set(mut self, key: String, value: Option<String>) -> Self {
        self.inner.insert(key, value);
        self
    }

    /// Replaces a key with a value on the server.
    ///
    /// This is the same as calling `set(key, Some(value))`.
    pub fn replace(self, key: String, value: String) -> Self {
        self.set(key, Some(value))
    }

    /// Deletes a key on the server.
    ///
    /// This is the same as calling `set(key, None)`.
    pub fn delete(self, key: String) -> Self {
        self.set(key, None)
    }

    /// Makes a key unchanged on the server.
    ///
    /// This differs from [`delete`][`Self::delete`] because internally, this removes the key from
    /// the update map, while [`delete`][`Self::delete`] sets the key to [`None`].
    pub fn unchange(mut self, key: String) -> Self {
        self.inner.remove(&key);
        self
    }

    builder!("persistent": [
        /// Sets the server-recognized `persistent` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_persistent,
        /// Replaces the server-recognized `persistent` property.
        ///
        /// See the [`replace`][`Self::replace`] documentation for more info.
        fn replace_persistent,
        /// Deletes the server-recognized `persistent` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_persistent,
        /// Makes the server-recognized `persistent` property unchanged.
        ///
        /// See the [`unchange`][`Self::unchange`] documentation for more info.
        fn unchange_persistent,
    ]: bool);

    builder!("retained": [
        /// Sets the server-recognized `retained` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_retained,
        /// Replaces the server-recognized `retained` property.
        ///
        /// See the [`replace`][`Self::replace`] documentation for more info.
        fn replace_retained,
        /// Deletes the server-recognized `retained` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_retained,
        /// Makes the server-recognized `retained` property unchanged.
        ///
        /// See the [`unchange`][`Self::unchange`] documentation for more info.
        fn unchange_retained,
    ]: bool);

    builder!("cached": [
        /// Sets the server-recognized `cached` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_cached,
        /// Replaces the server-recognized `cached` property.
        ///
        /// See the [`replace`][`Self::replace`] documentation for more info.
        fn replace_cached,
        /// Deletes the server-recognized `cached` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_cached,
        /// Makes the server-recognized `cached` property unchanged.
        ///
        /// See the [`unchange`][`Self::unchange`] documentation for more info.
        fn unchange_cached,
    ]: bool);

    /// Builds into a map used when updating topic properties.
    pub fn build(self) -> HashMap<String, Option<String>> {
        self.inner
    }
}

impl From<HashMap<String, Option<String>>> for SetPropsBuilder {
    fn from(value: HashMap<String, Option<String>>) -> Self {
        Self { inner: value }
    }
}

