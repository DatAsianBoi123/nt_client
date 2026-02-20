//! Topic publishers.
//!
//! Publishers are used to set new values for topics that can be seen by subscribers.
//!
//! # Examples
//!
//! **Using a [`Publisher`]:**
//! ```no_run
//! use std::time::Duration;
//! use nt_client::Client;
//!
//! # tokio_test::block_on(async {
//! let client = Client::new(Default::default());
//!
//! client.connect_setup(setup).await.unwrap();
//! # });
//!
//! fn setup(client: &Client) {
//!     // increments the `/counter` topic every 5 seconds
//!     let counter_topic = client.topic("/counter");
//!     tokio::spawn(async move {
//!         const INCREMENT_INTERVAL: Duration = Duration::from_secs(5);
//!     
//!         let mut publisher = counter_topic.publish::<u32>(Default::default()).await.unwrap();
//!         let mut interval = tokio::time::interval(INCREMENT_INTERVAL);
//!         let mut counter = 0;
//!     
//!         loop {
//!             interval.tick().await;
//!
//!             publisher.set(counter).await.expect("connection is still alive");
//!             counter += 1;
//!         }
//!     });
//! }
//! ```
//!
//! **Using a [`GenericPublisher`]:**
//! ```no_run
//! use std::time::Duration;
//! use nt_client::{data::DataType, Client};
//!
//! # tokio_test::block_on(async {
//! let client = Client::new(Default::default());
//!
//! client.connect_setup(setup).await.unwrap();
//! # });
//!
//! fn setup(client: &Client) {
//!     // increments the `/counter` topic every 5 seconds
//!     let counter_topic = client.topic("/counter");
//!     tokio::spawn(async move {
//!         const INCREMENT_INTERVAL: Duration = Duration::from_secs(5);
//!     
//!         let mut publisher = counter_topic.generic_publish(DataType::Int, Default::default()).await.unwrap();
//!         let mut interval = tokio::time::interval(INCREMENT_INTERVAL);
//!         let mut counter = 0;
//!     
//!         loop {
//!             interval.tick().await;
//!
//!             publisher.set(counter).await.expect("connection is still alive");
//!             counter += 1;
//!         }
//!     });
//! }
//! ```

use std::{collections::HashMap, fmt::Debug, marker::PhantomData, sync::Arc, time::Duration};

use tokio::sync::{broadcast, RwLock};

use crate::{NTClientReceiver, NTServerSender, NetworkTablesTime, data::{DataType, NetworkTableData}, error::ConnectionClosedError, net::{Announce, BinaryData, ClientboundData, ClientboundTextData, PropertiesData, Publish, ServerboundMessage, ServerboundTextData, SetProperties, Unpublish}, recv_until, topic::Properties};

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Publisher<T: NetworkTableData> {
    _phantom: PhantomData<T>,
    inner: GenericPublisher,
}

impl<T: NetworkTableData> Debug for Publisher<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher")
            .field("id", &self.inner.id)
            .field("type", &T::data_type())
            .finish()
    }
}

impl<T: NetworkTableData> PartialEq for Publisher<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: NetworkTableData> Eq for Publisher<T> { }

impl<T: NetworkTableData> Publisher<T> {
    pub(super) async fn new(
        name: String,
        properties: Properties,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        ws_recv: NTClientReceiver,
    ) -> Result<Self, NewPublisherError> {
        Ok(Self {
            _phantom: PhantomData,
            inner: GenericPublisher::new(name, properties, T::data_type(), time, ws_sender, ws_recv).await?,
        })
    }

    #[cfg(feature = "publish_bypass")]
    pub(super) async fn new_bypass(
        name: String,
        properties: Properties,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        ws_recv: NTClientReceiver,
    ) -> Result<Self, ConnectionClosedError> {
        Ok(Self {
            _phantom: PhantomData,
            inner: GenericPublisher::new_bypass(name, properties, T::data_type(), time, ws_sender, ws_recv).await?,
        })
    }

    /// Unpublishes from this topic.
    ///
    /// This behaves exactly the same as [std::mem::drop]ping this publisher, since the destructor
    /// for [`Publisher`]s automatically unpublishes.
    pub fn unpublish(self) { }

    /// Returns the id of this publisher.
    pub fn id(&self) -> i32 {
        self.inner.id()
    }

    /// Returns the data type of the topic this publisher is publishing to.
    pub fn data_type(&self) -> &DataType {
        &self.inner.r#type
    }

    /// Publish a new value to the [`Topic`].
    ///
    /// # Errors
    /// Returns an error if the client is disconnected.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set(&self, value: T) -> Result<(), ConnectionClosedError> {
        match self.inner.set(value).await {
            Ok(()) => Ok(()),
            Err(GenericPublishError::ConnectionClosed(_)) => Err(ConnectionClosedError),
            Err(GenericPublishError::MismatchedType { .. }) => unreachable!(),
        }
    }

    /// Publishes a default value to the [`Topic`].
    ///
    /// This default value will only be seen by other clients and the server if no other value has
    /// been published to the [`Topic`] yet.
    ///
    /// # Errors
    /// Returns an error if the client is disconnected.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set_default(&self, value: T) -> Result<(), ConnectionClosedError> {
        match self.inner.set_default(value).await {
            Ok(()) => Ok(()),
            Err(GenericPublishError::ConnectionClosed(_)) => Err(ConnectionClosedError),
            Err(GenericPublishError::MismatchedType { .. }) => unreachable!(),
        }
    }

    /// Updates the properties of the topic being subscribed to, returning a `future` that
    /// completes when the server acknowledges the update.
    ///
    /// # Errors
    /// Returns an error if messages could not be received from the `NetworkTables` server.
    ///
    /// # Examples
    /// ```no_run
    /// use std::time::Duration;
    /// use nt_client::{publish::UpdateProps, Client};
    ///
    /// # tokio_test::block_on(async {
    /// let client = Client::new(Default::default());
    ///
    /// client.connect_setup(setup).await.unwrap();
    /// # });
    ///
    /// fn setup(client: &Client) {
    ///     let topic = client.topic("mytopic");
    ///     tokio::spawn(async move {
    ///         let mut publisher = topic.publish::<String>(Default::default()).await.unwrap();
    ///
    ///         // update properties after 5 seconds
    ///         tokio::time::sleep(Duration::from_secs(5)).await;
    ///
    ///         // Props:
    ///         // - set `retained` to true
    ///         // - delete `arbitrary property`
    ///         // everything else stays unchanged
    ///         let props = UpdateProps::new()
    ///             .set_retained(true)
    ///             .delete("arbitrary property".to_owned());
    ///
    ///         publisher.update_props(props).await.unwrap();
    ///     });
    /// }
    /// ```
    pub async fn update_props(&mut self, new_props: UpdateProps) -> Result<(), broadcast::error::RecvError> {
        self.inner.update_props(new_props).await
    }
}

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This behaves different from [`Publisher`], as that has a generic type and
/// guarantees through type-safety that the client is publishing values that have the same type
/// the server does for that topic. Extra care must be taken to ensure no type mismatches
/// occur.
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct GenericPublisher {
    topic: String,
    id: i32,
    r#type: DataType,
    time: Arc<RwLock<NetworkTablesTime>>,
    ws_sender: NTServerSender,
    ws_recv: NTClientReceiver,
}

impl Debug for GenericPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericPublisher")
            .field("id", &self.id)
            .field("type", &self.r#type)
            .finish()
    }
}

impl PartialEq for GenericPublisher {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for GenericPublisher { }

impl GenericPublisher {
    pub(super) async fn new(
        name: String,
        properties: Properties,
        r#type: DataType,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        mut ws_recv: NTClientReceiver,
    ) -> Result<Self, NewPublisherError> {
        let id = rand::random();
        let pub_message = ServerboundTextData::Publish(Publish { name, pubuid: id, r#type: r#type.clone(), properties });
        ws_sender.send(ServerboundMessage::Text(pub_message)).map_err(|_| broadcast::error::RecvError::Closed)?;

        let (name, server_type, id) = {
            recv_until(&mut ws_recv, |data| {
                if let ClientboundData::Text(ClientboundTextData::Announce(Announce { ref name, ref r#type, pubuid: Some(pubuid), .. })) = *data {
                    Some((name.clone(), r#type.clone(), pubuid))
                } else {
                    None
                }
            }).await
        }?;

        if r#type != server_type {
            let data = ServerboundTextData::Unpublish(Unpublish { pubuid: id });
            // if the receiver is dropped, the ws connection is closed
            let _ = ws_sender.send(ServerboundMessage::Text(data));
            return Err(NewPublisherError::MismatchedType { server: server_type, client: r#type });
        };

        Ok(Self { topic: name, id, r#type, time, ws_sender, ws_recv })
    }

    #[cfg(feature = "publish_bypass")]
    pub(super) async fn new_bypass(
        name: String,
        properties: Properties,
        r#type: DataType,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        ws_recv: NTClientReceiver,
    ) -> Result<Self, ConnectionClosedError> {
        let id = rand::random();
        let pub_message = ServerboundTextData::Publish(Publish { name: name.clone(), pubuid: id, r#type: r#type.clone(), properties });
        ws_sender.send(ServerboundMessage::Text(pub_message)).map_err(|_| ConnectionClosedError)?;

        // assume the publisher will be made within 0.1s
        tokio::time::sleep(Duration::from_secs_f64(0.1)).await;

        Ok(Self { topic: name, id, r#type, time, ws_sender, ws_recv })
    }

    /// Unpublishes from this topic.
    ///
    /// This behaves exactly the same as [std::mem::drop]ping this publisher, since the destructor
    /// for [`GenericPublisher`]s automatically unpublishes.
    pub fn unpublish(self) { }

    /// Returns the id of this publisher.
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Returns the data type of the topic this publisher is publishing to.
    pub fn data_type(&self) -> &DataType {
        &self.r#type
    }

    /// Publish a new value to the [`Topic`].
    ///
    /// # Errors
    /// Returns an error if the client is disconnected or if there is a type mismatch between the
    /// server and client.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set<T: NetworkTableData>(&self, value: T) -> Result<(), GenericPublishError> {
        let time = self.time.read().await;
        self.set_time(value, time.server_time()).await
    }

    /// Publishes a default value to the [`Topic`].
    ///
    /// This default value will only be seen by other clients and the server if no other value has
    /// been published to the [`Topic`] yet.
    ///
    /// # Errors
    /// Returns an error if the client is disconnected or if there is a type mismatch between the
    /// server and client.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set_default<T: NetworkTableData>(&self, value: T) -> Result<(), GenericPublishError> {
        self.set_time(value, Duration::ZERO).await
    }

    /// Updates the properties of the topic being subscribed to, returning a `future` that
    /// completes when the server acknowledges the update.
    ///
    /// # Errors
    /// Returns an error if messages could not be received from the `NetworkTables` server.
    ///
    /// # Examples
    /// ```no_run
    /// use std::time::Duration;
    /// use nt_client::{publish::UpdateProps, data::DataType, Client};
    ///
    /// # tokio_test::block_on(async {
    /// let client = Client::new(Default::default());
    ///
    /// client.connect_setup(setup).await.unwrap();
    /// # });
    ///
    /// fn setup(client: &Client) {
    ///     let topic = client.topic("mytopic");
    ///     tokio::spawn(async move {
    ///         let mut publisher = topic.generic_publish(DataType::String, Default::default()).await.unwrap();
    ///
    ///         // update properties after 5 seconds
    ///         tokio::time::sleep(Duration::from_secs(5)).await;
    ///
    ///         // Props:
    ///         // - set `retained` to true
    ///         // - delete `arbitrary property`
    ///         // everything else stays unchanged
    ///         let props = UpdateProps::new()
    ///             .set_retained(true)
    ///             .delete("arbitrary property".to_owned());
    ///
    ///         publisher.update_props(props).await.unwrap();
    ///     });
    /// }
    /// ```
    pub async fn update_props(&mut self, new_props: UpdateProps) -> Result<(), broadcast::error::RecvError> {
        self.ws_sender.send(ServerboundMessage::Text(ServerboundTextData::SetProperties(SetProperties {
            name: self.topic.clone(),
            update: new_props.into(),
        }))).map_err(|_| broadcast::error::RecvError::Closed)?;

        recv_until(&mut self.ws_recv, |data| {
            if let ClientboundData::Text(ClientboundTextData::Properties(PropertiesData { ref name, .. })) = *data {
                if name != &self.topic { return None; };

                Some(())
            } else {
                None
            }
        }).await?;

        Ok(())
    }

    async fn set_time<T: NetworkTableData>(&self, data: T, timestamp: Duration) -> Result<(), GenericPublishError> {
        if self.r#type != T::data_type() {
            return Err(GenericPublishError::MismatchedType { server: self.r#type.clone(), client: T::data_type() });
        };

        let binary = BinaryData::new(self.id, timestamp, data);
        self.ws_sender.send(ServerboundMessage::Binary(binary)).map_err(|_| ConnectionClosedError)?;
        Ok(())
    }
}

impl Drop for GenericPublisher {
    fn drop(&mut self) {
        let data = ServerboundTextData::Unpublish(Unpublish { pubuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(data));
    }
}

/// Errors that can occur when creating a new [`Publisher`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum NewPublisherError {
    /// An error occurred when receiving data from the connection.
    #[error(transparent)]
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

/// Errors that can occur when generically publishing using a [`GenericPublisher`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum GenericPublishError {
    /// The server and client have mismatched data types.
    ///
    /// This can occur if, for example, the client publishes a [`String`] to a topic that the
    /// server has a different data type for, like an [`i32`].
    #[error("mismatched data types! server has {server:?}, but tried to use {client:?} instead")]
    MismatchedType {
        /// The server's data type.
        server: DataType,
        /// The client's data type.
        client: DataType,
    },
    /// The `NetworkTables` connection was closed.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),
}

macro_rules! builder {
    ($lit: literal : [
        $( #[ $gc_m: meta ] )* fn $get_ckd: ident,
        $( #[ $g_m: meta ] )* fn $get: ident,
        $( #[ $u_m: meta ] )* fn $update: ident,
        $( #[ $s_m: meta ] )* fn $set: ident,
        $( #[ $d_m: meta ] )* fn $delete: ident,
        $( #[ $k_m: meta ] )* fn $keep: ident,
    ] : $ty: ty where
        $as_pat: pat => $as_value: expr,
        $from_pat: pat => $from_value: expr) => {
        $( #[ $gc_m ] )*
        pub fn $get_ckd(&self) -> Option<PropUpdate<&$ty>> {
            match self.get($lit) {
                PropUpdate::Set($from_pat) => Some(PropUpdate::Set($from_value)),
                PropUpdate::Set(_) => None,
                PropUpdate::Delete => Some(PropUpdate::Delete),
                PropUpdate::Keep => Some(PropUpdate::Keep),
            }
        }

        $( #[ $g_m ] )*
        pub fn $get(&self) -> PropUpdate<&$ty> {
            match self.$get_ckd() {
                Some(value) => value,
                None => panic!("invalid `{}` value", $lit)
            }
        }

        $( #[ $u_m ] )*
        pub fn $update(self, value: PropUpdate<$ty>) -> Self {
            let value = match value {
                PropUpdate::Set($as_pat) => PropUpdate::Set($as_value),
                PropUpdate::Delete => PropUpdate::Delete,
                PropUpdate::Keep => PropUpdate::Keep,
            };
            self.update($lit.to_owned(), value)
        }
        $( #[ $s_m ] )*
        pub fn $set(self, value: $ty) -> Self {
            let $as_pat = value;
            let value = $as_value;
            self.set($lit.to_owned(), value)
        }
        $( #[ $d_m ] )*
        pub fn $delete(self) -> Self {
            self.delete($lit.to_owned())
        }
        $( #[ $k_m ] )*
        pub fn $keep(self) -> Self {
            self.keep($lit.to_owned())
        }
    };
}

/// Properties to update on a topic.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct UpdateProps {
    inner: HashMap<String, Option<serde_json::Value>>,
}

impl UpdateProps {
    /// Creates a new property update. All properties are `kept` by default.
    ///
    /// This is identical to calling [`Default::default`].
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a property update updating certain server-recognized properties.
    ///
    /// This method differs from [`with_props_keep`][`Self::with_props_keep`] because
    /// unlike that method, this method deletes the property if it's [`None`].
    ///
    /// With the `extra` field, if the key is not present in the map, it does not get updated. If
    /// the key is present, it gets set to the value.
    ///
    /// Behavior with the `extra` field is identical in both methods.
    ///
    /// # Examples
    /// ```
    /// use nt_client::{publish::UpdateProps, topic::Properties};
    ///
    /// // properties are:
    /// // - persistent: `true`
    /// // - cached: `true`
    /// // - retained: None (defaults to `false`)
    /// let properties = Properties { persistent: Some(true), cached: Some(true), ..Default::default() };
    ///
    /// // update properties is:
    /// // - set `persistent` to `true`
    /// // - set `cached` to `true`
    /// // - delete `retained`
    /// // everything else stays unchanged
    /// let builder = UpdateProps::with_props_delete(properties);
    /// ```
    pub fn with_props_delete(Properties { persistent, retained, cached, extra }: Properties) -> Self {
        let mut update = Self::new()
            .update_persistent(PropUpdate::from_option_delete(persistent))
            .update_retained(PropUpdate::from_option_delete(retained))
            .update_cached(PropUpdate::from_option_delete(cached));

        for (key, value) in extra {
            update = update.set(key, value);
        }

        update
    }

    /// Creates a new property update updating certain server-recognized properties.
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
    /// use nt_client::{publish::UpdateProps, topic::Properties};
    ///
    /// // properties are:
    /// // - persistent: `true`
    /// // - cached: `true`
    /// // - retained: None (defaults to `false`)
    /// let properties = Properties { persistent: Some(true), cached: Some(true), ..Default::default() };
    ///
    /// // update properties is:
    /// // - set `persistent` to `true`
    /// // - set `cached` to `true`
    /// // everything else stays unchanged
    /// let builder = UpdateProps::with_props_keep(properties);
    /// ```
    pub fn with_props_keep(Properties { persistent, retained, cached, extra }: Properties) -> Self {
        let mut update = Self::new()
            .update_persistent(PropUpdate::from_option_keep(persistent))
            .update_retained(PropUpdate::from_option_keep(retained))
            .update_cached(PropUpdate::from_option_keep(cached));

        for (key, value) in extra {
            update = update.set(key, value);
        }

        update
    }

    /// Gets how a property will update.
    pub fn get(&self, key: &str) -> PropUpdate<&serde_json::Value> {
        match self.inner.get(key) {
            Some(Some(value)) => PropUpdate::Set(value),
            Some(None) => PropUpdate::Delete,
            None => PropUpdate::Keep,
        }
    }

    /// Updates a property.
    /// 
    /// See also:
    /// - [`set`][`Self::set`]
    /// - [`delete`][`Self::delete`]
    /// - [`keep`][`Self::keep`]
    pub fn update(mut self, key: String, update: PropUpdate<serde_json::Value>) -> Self {
        match update {
            PropUpdate::Set(value) => self.inner.insert(key, Some(value)),
            PropUpdate::Delete => self.inner.insert(key, None),
            PropUpdate::Keep => self.inner.remove(&key),
        };
        self
    }

    /// Sets a new value to the property.
    ///
    /// This is the same as calling `update(key, PropUpdate::Set(value))`.
    pub fn set(self, key: String, value: serde_json::Value) -> Self {
        self.update(key, PropUpdate::Set(value))
    }

    /// Deletes a property on the server.
    ///
    /// This is the same as calling `update(key, PropUpdate::Delete)`.
    pub fn delete(self, key: String) -> Self {
        self.update(key, PropUpdate::Delete)
    }

    /// Keeps a property on the server, leaving its current value unchanged.
    ///
    /// By default, all properties are `kept`.
    ///
    /// This is the same as calling `update(key, PropUpdate::Keep)`.
    pub fn keep(self, key: String) -> Self {
        self.update(key, PropUpdate::Keep)
    }

    builder!("persistent": [
        /// Gets the server-recognized `persistent` property, checking whether it is a [`bool`] or
        /// not.
        ///
        /// A return value of [`None`] indicates that the `persistent` property is not a [`bool`].
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// This is the checked version of [`persistent`][`Self::persistent`].
        fn persistent_checked,
        /// Gets the server-recognized `persistent` property.
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// # Panics
        ///
        /// Panics if the `persistent` property is not set to a boolean.
        fn persistent,
        /// Updates the server-recognized `persistent` property.
        ///
        /// See the [`update`][`Self::update`] documentation for more info.
        fn update_persistent,
        /// Sets the server-recognized `persistent` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_persistent,
        /// Deletes the server-recognized `persistent` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_persistent,
        /// Keeps the server-recognized `persistent` property.
        ///
        /// See the [`keep`][`Self::keep`] documentation for more info.
        fn keep_persistent,
    ]: bool where
        bool => serde_json::Value::Bool(bool),
        serde_json::Value::Bool(value) => value);

    builder!("retained": [
        /// Gets the server-recognized `retained` property, checking whether it is a [`bool`] or
        /// not.
        ///
        /// A return value of [`None`] indicates that the `retained` property is not a [`bool`].
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// This is the checked version of [`retained`][`Self::retained`].
        fn retained_checked,
        /// Gets the server-recognized `retained` property.
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// # Panics
        ///
        /// Panics if the `retained` property is not set to a boolean.
        fn retained,
        /// Updates the server-recognized `retained` property.
        ///
        /// See the [`update`][`Self::update`] documentation for more info.
        fn update_retained,
        /// Sets the server-recognized `retained` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_retained,
        /// Deletes the server-recognized `retained` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_retained,
        /// Keeps the server-recognized `retained` property.
        ///
        /// See the [`keep`][`Self::keep`] documentation for more info.
        fn keep_retained,
    ]: bool where
        bool => serde_json::Value::Bool(bool),
        serde_json::Value::Bool(value) => value);


    builder!("cached": [
        /// Gets the server-recognized `cached` property, checking whether it is a [`bool`] or
        /// not.
        ///
        /// A return value of [`None`] indicates that the `cached` property is not a [`bool`].
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// This is the checked version of [`cached`][`Self::cached`].
        fn cached_checked,
        /// Gets the server-recognized `cached` property.
        ///
        /// See the [`get`][`Self::get`] documentation for more info.
        ///
        /// # Panics
        ///
        /// Panics if the `cached` property is not set to a boolean.
        fn cached,
        /// Updates the server-recognized `cached` property.
        ///
        /// See the [`update`][`Self::update`] documentation for more info.
        fn update_cached,
        /// Sets the server-recognized `cached` property.
        ///
        /// See the [`set`][`Self::set`] documentation for more info.
        fn set_cached,
        /// Deletes the server-recognized `cached` property.
        ///
        /// See the [`delete`][`Self::delete`] documentation for more info.
        fn delete_cached,
        /// Keeps the server-recognized `cached` property.
        ///
        /// See the [`keep`][`Self::keep`] documentation for more info.
        fn keep_cached,
    ]: bool where
        bool => serde_json::Value::Bool(bool),
        serde_json::Value::Bool(value) => value);
}

impl From<HashMap<String, Option<serde_json::Value>>> for UpdateProps {
    fn from(value: HashMap<String, Option<serde_json::Value>>) -> Self {
        Self { inner: value }
    }
}

impl From<UpdateProps> for HashMap<String, Option<serde_json::Value>> {
    fn from(value: UpdateProps) -> Self {
        value.inner
    }
}

/// An update to a property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropUpdate<T> {
    /// Sets a new value to the property.
    Set(T),
    /// Deletes the property.
    Delete,
    /// Keeps the property, leaving its value unchanged.
    Keep,
}

impl<T> PropUpdate<T> {
    /// Converts an `Option<T>` to a `PropUpdate<T>`, deleting the property if it is [`None`].
    pub fn from_option_delete(option: Option<T>) -> Self {
        match option {
            Some(t) => Self::Set(t),
            None => Self::Delete,
        }
    }

    /// Converts an `Option<T>` to a `PropUpdate<T>`, keeping the property if it is [`None`].
    pub fn from_option_keep(option: Option<T>) -> Self {
        match option {
            Some(t) => Self::Set(t),
            None => Self::Keep,
        }
    }
}

