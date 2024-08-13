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
//!         publisher.set(counter).await;
//!         counter += 1;
//!     }
//! });
//!
//! client.connect().await.unwrap();
//! # });

use std::{marker::PhantomData, sync::Arc, time::Duration};

use tokio::{sync::{broadcast, RwLock}, time::timeout};
use tracing::debug;

use crate::{data::{r#type::{DataType, NetworkTableData}, Announce, BinaryData, ClientboundData, ClientboundTextData, Properties, Publish, ServerboundMessage, ServerboundTextData, Unpublish}, recv_until, NTClientReceiver, NTServerSender, NetworkTablesTime};

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
#[derive(Debug)]
pub struct Publisher<T: NetworkTableData> {
    _phantom: PhantomData<T>,
    id: i32,
    time: Arc<RwLock<NetworkTablesTime>>,
    ws_sender: NTServerSender,
}

impl<T: NetworkTableData> PartialEq for Publisher<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T: NetworkTableData> Eq for Publisher<T> { }

impl<T: NetworkTableData> Publisher<T> {
    // NOTE: pub(super) or pub?
    pub(super) async fn new(
        name: String,
        properties: Properties,
        time: Arc<RwLock<NetworkTablesTime>>,
        timeout_duration: Duration,
        ws_sender: NTServerSender,
        mut ws_recv: NTClientReceiver,
    ) -> Result<Self, NewPublisherError> {
        let id = rand::random();
        let pub_message = ServerboundTextData::Publish(Publish { name, pubuid: id, r#type: T::data_type(), properties });
        ws_sender.send(ServerboundMessage::Text(pub_message).into()).map_err(|_| broadcast::error::RecvError::Closed)?;

        let (name, r#type, id) = timeout(timeout_duration, async move {
            recv_until(&mut ws_recv, |data| {
                if let ClientboundData::Text(ClientboundTextData::Announce(Announce { ref name, ref r#type, pubuid: Some(pubuid), .. })) = *data {
                    // TODO: cached property

                    Some((name.clone(), r#type.clone(), pubuid))
                } else {
                    None
                }
            }).await
        }).await.map_err(|_| NewPublisherError::NoResponse)??;
        if T::data_type() != r#type { return Err(NewPublisherError::MismatchedType { server: r#type, client: T::data_type() }); };

        debug!("[pub {id}] publishing to topic `{name}`");
        Ok(Self { _phantom: PhantomData, id, time, ws_sender })
    }

    /// Publish a new value to the [`Topic`].
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set(&self, value: T) {
        let time = self.time.read().await;
        self.set_time(value, time.server_time()).await
    }

    /// Publishes a default value to the [`Topic`].
    ///
    /// This default value will only be seen by other clients and the server if no other value has
    /// been published to the [`Topic`] yet.
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set_default(&self, value: T) {
        self.set_time(value, Duration::ZERO).await
    }

    async fn set_time(&self, data: T, timestamp: Duration) {
        let data_value = data.clone().into_value();
        let binary = BinaryData::new(self.id, timestamp, data);
        self.ws_sender.send(ServerboundMessage::Binary(binary).into()).expect("receiver still exists");
        debug!("[pub {}] set to {data_value} at time {timestamp:?}", self.id);
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
    Recv(#[from] broadcast::error::RecvError),
    /// The server didn't respond to the publish within the response timeout specified in [`NewClientOptions`].
    ///
    /// [`NewClientOptions`]: crate::NewClientOptions
    #[error("server didn't respond with announce")]
    NoResponse,
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

