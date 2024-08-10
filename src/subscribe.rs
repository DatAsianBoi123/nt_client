//! Subscriber portion of the `NetworkTables` spec.
//!
//! Subscribers receive data value updates to a topic.
//!
//! # Examples
//!
//! ```no_run
//! use nt_client::Client;
//!
//! # tokio_test::block_on(async {
//! let client = Client::new(Default::default());
//!
//! // prints updates to the `/counter` topic to the stdout
//! let counter_topic = client.topic("/counter");
//! tokio::spawn(async move {
//!     // subscribes to the `/counter` topic and expects the type to be `i32`
//!     let mut subscriber = counter_topic.subscribe::<i32>(Default::default()).await.unwrap();
//!     
//!     while let Ok(recv) = subscriber.recv().await {
//!         println!("{recv}");
//!     }
//! });
//!
//! client.connect().await.unwrap();
//! # });

use std::{marker::PhantomData, time::Duration};

use tokio::sync::broadcast;
use tracing::debug;

use crate::{data::{r#type::{DataType, NetworkTableData}, Announce, BinaryData, ClientboundData, ClientboundTextData, ServerboundMessage, ServerboundTextData, Subscribe, SubscriptionOptions, Unsubscribe}, recv_until, NTClientReceiver, NTServerSender};

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

// FIX: multiple topics being subscribed to causes bugs
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

        let (r#type, topic_id) = recv_until(&mut ws_recv, |data| {
            if let ClientboundData::Text(ClientboundTextData::Announce(Announce { id, ref r#type, .. })) = *data {
                // TODO: handle other properties

                Some((r#type.clone(), id))
            } else { None }
        }).await?;
        if T::data_type() != r#type { return Err(NewSubscriberError::MismatchedType { server: r#type, client: T::data_type() }); };

        debug!("[sub {id}] subscribed");
        Ok(Self { _phantom: PhantomData, id, topic_id, prev_timestamp: None, ws_sender, ws_recv })
    }

    /// Receives the next value for this subscriber.
    pub async fn recv(&mut self) -> Result<T, broadcast::error::RecvError> {
        recv_until(&mut self.ws_recv, |data| {
            if let ClientboundData::Binary(BinaryData { id, ref timestamp, ref data, .. }) = *data {
                if id != self.topic_id { return None; };
                let past = if let Some(last_timestamp) = self.prev_timestamp { last_timestamp > *timestamp } else { false };
                if past { return None; };

                self.prev_timestamp = Some(*timestamp);
                debug!("[sub {}] updated: {data}", self.id);
                Some(T::from_value(data).expect("types match up"))
            } else {
                None
            }
        }).await
    }

    // TODO: update method
}

impl<T: NetworkTableData> Drop for Subscriber<T> {
    fn drop(&mut self) {
        let unsub_message = ServerboundTextData::Unsubscribe(Unsubscribe { subuid: self.id });
        // if the receiver is dropped, the ws connection is closed
        let _ = self.ws_sender.send(ServerboundMessage::Text(unsub_message).into());
        debug!("[sub {}] unsubscribed", self.id);
    }
}

/// Errors that can occur when creating a new [`Subscriber`].
#[derive(thiserror::Error, Debug)]
pub enum NewSubscriberError {
    /// An error occurred when receiving data from the connection.
    #[error(transparent)]
    Recv(#[from] broadcast::error::RecvError),
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

