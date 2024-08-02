use std::{marker::PhantomData, sync::Arc, time::Duration};

use tokio::{sync::RwLock, time::timeout};

use crate::{dataframe::{datatype::{DataType, NetworkTableDataType}, Announce, BinaryData, ClientboundData, ClientboundTextData, Properties, Publish, ServerboundMessage, ServerboundTextData, Unpublish}, error::ConnectionClosedError, NTClientReceiver, NTServerSender, NetworkTablesTime};

/// A `NetworkTables` publisher that publishes values to a [`Topic`].
///
/// This will automatically get unpublished whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Publisher<T: NetworkTableDataType> {
    _phantom: PhantomData<T>,
    id: i32,
    time: Arc<RwLock<NetworkTablesTime>>,
    ws_sender: NTServerSender,
}

impl<T: NetworkTableDataType> Publisher<T> {
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

    pub async fn set(&self, value: T) {
        let time = self.time.write().await;
        self.set_time(value, time.server_time()).await
    }

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

impl<T: NetworkTableDataType> Drop for Publisher<T> {
    fn drop(&mut self) {
        let data = ServerboundTextData::Unpublish(Unpublish { pubuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(data).into());
        println!("[pub {}] unsubscribed", self.id);
    }
}

#[derive(thiserror::Error, Debug)]
pub enum NewPublisherError {
    #[error("server didn't respond with announce")]
    NoResponse,
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),
    #[error("mismatched data types! server has {0:?}, but tried to use {1:?} instead")]
    MismatchedType(DataType, DataType),
}

