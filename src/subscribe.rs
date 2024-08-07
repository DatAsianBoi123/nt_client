use std::{marker::PhantomData, time::Duration};

use crate::{dataframe::{datatype::{DataType, NetworkTableData}, Announce, BinaryData, ClientboundData, ClientboundTextData, ServerboundMessage, ServerboundTextData, Subscribe, SubscriptionOptions, Unsubscribe}, recv_until, NTClientReceiver, NTServerSender};

/// A `NetworkTables` subscriber that subscribes to a [`Topic`].
///
/// This will automatically get unsubscribed whenever this goes out of scope.
///
/// [`Topic`]: crate::topic::Topic
pub struct Subscriber<T: NetworkTableData> {
    _phantom: PhantomData<T>,
    id: i32,
    topic_id: i32,
    prev_timestamp: Option<Duration>,

    ws_sender: NTServerSender,
    ws_recv: NTClientReceiver,
}

// TODO: handle multiple topics being subscribed to
impl<T: NetworkTableData> Subscriber<T> {
    // NOTE: pub(super) or pub?
    pub(super) async fn new(
        topics: Vec<String>,
        options: SubscriptionOptions,
        ws_sender: NTServerSender,
        mut ws_recv: NTClientReceiver,
    ) -> Result<Self, NewSubscriberError> {
        let id = rand::random();
        let sub_message = ServerboundTextData::Subscribe(Subscribe { topics, subuid: id, options });
        ws_sender.send(ServerboundMessage::Text(sub_message).into()).expect("receivers exist");

        let (r#type, topic_id) = Self::wait_for_response(&mut ws_recv).await?;
        if T::data_type() != r#type { return Err(NewSubscriberError::MismatchedType(r#type, T::data_type())); };

        Ok(Self { _phantom: PhantomData, id, topic_id, prev_timestamp: None, ws_sender, ws_recv })
    }

    /// Receives the next value for this subscriber.
    pub async fn recv(&mut self) -> Result<T, ConnectionClosedError> {
        while let Ok(data) = self.ws_recv.recv().await {
            match *data {
                ClientboundData::Binary(BinaryData { id, ref timestamp, ref data, .. }) => {
                    if id != self.topic_id { continue; };
                    let past = if let Some(last_timestamp) = self.prev_timestamp { last_timestamp > *timestamp } else { false };
                    if past { continue; };

                    self.prev_timestamp = Some(*timestamp);
                    return Ok(T::from_value(data).expect("types match up"));
                },
                _ => continue,
            };
        };
        // TODO: handle RecvError::Lagged
        Err(ConnectionClosedError)
    }

    async fn wait_for_response(recv_ws: &mut NTClientReceiver) -> Result<(DataType, i32), ConnectionClosedError> {
        while let Ok(data) = recv_ws.recv().await {
            match *data {
                ClientboundData::Text(ClientboundTextData::Announce(Announce { id, ref r#type, .. })) => {
                    // TODO: cached property

                    return Ok((r#type.clone(), id));
                },
                _ => continue,
            };
        };
        // TODO: handle RecvError::Lagged
        Err(ConnectionClosedError)
    }

    // TODO: update method
}

impl<T: NetworkTableData> Drop for Subscriber<T> {
    fn drop(&mut self) {
        let unsub_message = ServerboundTextData::Unsubscribe(Unsubscribe { subuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(unsub_message).into());
        println!("[sub {}] unsubscribed", self.id);
    }
}

/// Errors that can occur when creating a new [`Subscriber`].
#[derive(thiserror::Error, Debug)]
pub enum NewSubscriberError {
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),
    /// The server and client have mismatched data types.
    ///
    /// This can occur if, for example, the client is subscribing to a topic and expecting
    /// [`String`]s, but the server has a different data type stored, like an [`i32`].
    #[error("mismatched data types! server has {server:?}, but tried to use {client:?} instead")]
    MismatchedType {
        /// The server's data type.
        server: DataType,
        /// The client's data type.
        client: DataType,
    },
}

