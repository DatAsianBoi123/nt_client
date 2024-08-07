// TODO: example in doc

//! Publisher portion of the `NetworkTables` spec.
//!
//! Publishers are used to set new values for topics that can be seen by subscribers.

use std::{marker::PhantomData, sync::Arc, time::Duration};

use tokio::{sync::RwLock, time::timeout};

use crate::{dataframe::{datatype::{DataType, NetworkTableData}, Announce, BinaryData, ClientboundData, ClientboundTextData, Properties, Publish, ServerboundMessage, ServerboundTextData, Unpublish}, recv_until, NTClientReceiver, NTServerSender, NetworkTablesTime};

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Publisher<T: NetworkTableData> {
    _phantom: PhantomData<T>,
    id: i32,
    time: Arc<RwLock<NetworkTablesTime>>,
    ws_sender: NTServerSender,
}

impl<T: NetworkTableData> Publisher<T> {
    // NOTE: pub(super) or pub?
    pub(super) async fn new(
        name: String,
        properties: Properties,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        mut ws_recv: NTClientReceiver,
    ) -> Result<Self, NewPublisherError> {
        let id = rand::random();
        let pub_message = ServerboundTextData::Publish(Publish { name, pubuid: id, r#type: T::data_type(), properties });
        ws_sender.send(ServerboundMessage::Text(pub_message).into()).map_err(|_| ConnectionClosedError)?;

        let (r#type, id) = timeout(Duration::from_secs(1), Self::wait_for_response(&mut ws_recv)).await.map_err(|_| NewPublisherError::NoResponse)??;
        if T::data_type() != r#type { return Err(NewPublisherError::MismatchedType(r#type, T::data_type())); };

        Ok(Self { _phantom: PhantomData, id, time, ws_sender })
    }

    /// Publish a new value to the [`Topic`].
    ///
    /// [`Topic`]: crate::topic::Topic
    pub async fn set(&self, value: T) {
        let time = self.time.write().await;
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
        println!("[pub {}] set value to {data_value}", self.id)
    }

    async fn wait_for_response(recv_ws: &mut NTClientReceiver) -> Result<(DataType, i32), ConnectionClosedError> {
        while let Ok(data) = recv_ws.recv().await {
            match *data {
                ClientboundData::Text(ClientboundTextData::Announce(Announce { ref r#type, pubuid: Some(pubuid), .. })) => {
                    // TODO: cached property

                    return Ok((r#type.clone(), pubuid));
                },
                _ => continue,
            }
        }
        // TODO: handle RecvError::Lagged
        Err(ConnectionClosedError)
    }
}

impl<T: NetworkTableData> Drop for Publisher<T> {
    fn drop(&mut self) {
        let data = ServerboundTextData::Unpublish(Unpublish { pubuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(data).into());
        println!("[pub {}] unsubscribed", self.id);
    }
}

/// Errors that can occur when creating a new [`Publisher`].
#[derive(thiserror::Error, Debug)]
pub enum NewPublisherError {
    /// The server didn't respond to the publish within the response timeout specified in [`NewClientOptions`].
    ///
    /// [`NewClientOptions`]: crate::NewClientOptions
    #[error("server didn't respond with announce")]
    NoResponse,
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),
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

