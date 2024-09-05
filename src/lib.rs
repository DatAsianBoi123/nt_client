#![warn(missing_docs)]

//! A blazingly fast [NetworkTables 4.1][NetworkTables] client.
//!
//! Provides a client that can be used to interface with a [NetworkTables] server. This is
//! intended to be used within a coprocessor on the robot. Keep in mind that this is a pre-1.0.0
//! release, so many things may not work properly and expect breaking changes until a full 1.0.0
//! release is available.
//!
//! # Examples
//!
//! ```no_run
//! use nt_client::{subscribe::ReceivedMessage, Client, NewClientOptions, NTAddr};
//!
//! # tokio_test::block_on(async {
//! let options = NewClientOptions { addr: NTAddr::Local, ..Default::default() };
//! let client = Client::new(options);
//!
//! let thing_topic = client.topic("/thing");
//! tokio::spawn(async move {
//!     let mut sub = thing_topic.subscribe(Default::default()).await;
//!
//!     while let Ok(ReceivedMessage::Updated((_topic, value))) = sub.recv().await {
//!         println!("topic updated: '{value}'")
//!     }
//! });
//! 
//! client.connect().await.unwrap();
//! # });
//! ```
//! 
//! [NetworkTables]: https://github.com/wpilibsuite/allwpilib/blob/main/ntcore/doc/networktables4.adoc

use core::panic;
use std::{collections::{HashMap, VecDeque}, convert::Into, net::Ipv4Addr, sync::Arc, time::{Duration, Instant}};

use data::{BinaryData, ClientboundData, ClientboundTextData, ServerboundMessage, Unannounce};
use error::{ConnectError, ConnectionClosedError, IntoAddrError, PingError, ReceiveMessageError, SendMessageError, UpdateTimeError};
use futures_util::{stream::{SplitSink, SplitStream}, Future, SinkExt, StreamExt, TryStreamExt};
use time::ext::InstantExt;
use tokio::{net::TcpStream, select, sync::{broadcast, mpsc, Notify, RwLock}, task::JoinHandle, time::{interval, timeout}};
use tokio_tungstenite::{tungstenite::{self, http::{Response, Uri}, ClientRequestBuilder, Message}, MaybeTlsStream, WebSocketStream};
use topic::{AnnouncedTopic, Topic};
use tracing::{debug, error, info};

pub mod error;
pub mod data;
pub mod topic;
pub mod subscribe;
pub mod publish;

pub(crate) type NTServerSender = broadcast::Sender<Arc<ServerboundMessage>>;
pub(crate) type NTServerReceiver = broadcast::Receiver<Arc<ServerboundMessage>>;

pub(crate) type NTClientSender = broadcast::Sender<Arc<ClientboundData>>;
pub(crate) type NTClientReceiver = broadcast::Receiver<Arc<ClientboundData>>;

/// The client used to interact with a `NetworkTables` server.
///
/// When this goes out of scope, the websocket connection is closed and no attempts to reconnect
/// will be made.
#[derive(Debug)]
pub struct Client {
    addr: Ipv4Addr,
    options: NewClientOptions,
    time: Arc<RwLock<NetworkTablesTime>>,
    announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,

    send_ws: (NTServerSender, NTServerReceiver),
    recv_ws: (NTClientSender, NTClientReceiver),
}

impl Client {
    /// Creates a new `Client` with options.
    ///
    /// # Panics
    /// Panics if the [`NTAddr::TeamNumber`] team number is greater than 25599.
    pub fn new(options: NewClientOptions) -> Self {
        let addr = match options.addr.clone().into_addr() {
            Ok(addr) => addr,
            Err(err) => panic!("{err}"),
        };

        Client {
            addr,
            options,
            time: Default::default(),
            announced_topics: Default::default(),

            send_ws: broadcast::channel(10),
            recv_ws: broadcast::channel(20),
        }
    }

    /// Returns the current `NetworkTablesTime` for this client.
    ///
    /// This can safely be used asynchronously and across different threads.
    pub fn time(&self) -> Arc<RwLock<NetworkTablesTime>> {
        self.time.clone()
    }

    /// Returns a map of server announced topics for this client.
    ///
    /// This can be safely used asynchronously and across different threads.
    pub fn announced_topics(&self) -> Arc<RwLock<HashMap<i32, AnnouncedTopic>>> {
        self.announced_topics.clone()
    }

    /// Returns a new topic with a given name.
    pub fn topic(&self, name: impl ToString) -> Topic {
        Topic::new(name.to_string(), self.time(), self.announced_topics.clone(), self.send_ws.0.clone(), self.recv_ws.0.clone())
    }

    /// Connects to the `NetworkTables` server.
    ///
    /// This future will only complete when the client has disconnected from the server.
    pub async fn connect(self) -> Result<(), ConnectError> {
        let (ws_stream, _) = match self.try_connect("wss", self.options.secure_port).await {
            Ok(ok) => ok,
            Err(tungstenite::Error::Io(_)) => self.try_connect("ws", self.options.unsecure_port).await?,
            Err(err) => return Err(err.into()),
        };

        let (write, read) = ws_stream.split();

        let pong_notify_recv = Arc::new(Notify::new());
        let pong_notify_send = pong_notify_recv.clone();
        let ping_task = Client::start_ping_task(pong_notify_recv, self.send_ws.0.clone(), self.options.ping_interval, self.options.response_timeout);

        let (update_time_sender, update_time_recv) = mpsc::channel(1);
        let update_time_task = Client::start_update_time_task(self.options.update_time_interval, self.time(), self.send_ws.0.clone(), update_time_recv);

        let announced_topics = self.announced_topics();
        let write_task = Client::start_write_task(self.send_ws.1, write);
        let read_task = Client::start_read_task(read, update_time_sender, pong_notify_send, announced_topics, self.recv_ws.0);

        let result = select! {
            task = ping_task => task?.map_err(|err| err.into()),
            task = write_task => task?.map_err(|err| err.into()),
            task = read_task => task?.map_err(|err| err.into()),
            task = update_time_task => task?.map_err(|err| err.into()),
        };
        info!("closing connection");
        result
    }

    async fn try_connect(
        &self,
        scheme: &str,
        port: u16,
    ) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response<Option<Vec<u8>>>), tungstenite::Error> {
        let uri: Uri = format!("{scheme}://{}:{port}/nt/{}", self.addr, self.options.name).try_into().expect("valid websocket uri");
        let conn_str = uri.to_string();
        debug!("attempting connection at {conn_str}");
        let client_request = ClientRequestBuilder::new(uri)
            .with_sub_protocol("v4.1.networktables.first.wpi.edu");

        let res = tokio_tungstenite::connect_async(client_request).await;

        if res.is_ok() { info!("connected to server at {conn_str}") };

        res
    }

    fn start_ping_task(
        pong_recv: Arc<Notify>,
        ws_sender: NTServerSender,
        ping_interval: Duration,
        response_timeout: Duration,
    ) -> JoinHandle<Result<(), PingError>> {
        tokio::spawn(async move {
            let mut interval = interval(ping_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                ws_sender.send(ServerboundMessage::Ping.into()).map_err(|_| ConnectionClosedError)?;

                if (timeout(response_timeout, pong_recv.notified()).await).is_err() {
                    return Err(PingError::PongTimeout);
                }
            }
        })
    }

    fn start_update_time_task(
        update_time_interval: Duration,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        mut time_recv: mpsc::Receiver<(Duration, Duration)>,
    ) -> JoinHandle<Result<(), UpdateTimeError>> {
        tokio::spawn(async move {
            let mut interval = interval(update_time_interval);
            loop {
                interval.tick().await;

                let client_time = {
                    let time = time.read().await;
                    time.client_time()
                };
                // TODO: handle client time overflow
                let data = BinaryData::new::<u64>(
                    -1,
                    Duration::ZERO,
                    client_time.whole_microseconds().try_into().map_err(|_| UpdateTimeError::TimeOverflow)?,
                );
                ws_sender.send(ServerboundMessage::Binary(data).into()).map_err(|_| ConnectionClosedError)?;

                if let Some((timestamp, client_send_time)) = time_recv.recv().await {
                    let offset = {
                        let now = time.read().await.client_time();
                        let rtt = now - client_send_time;
                        let server_time = timestamp - rtt / 2;

                        server_time - now
                    };

                    let mut time = time.write().await;
                    time.offset = offset;
                    debug!("updated time, offset = {offset:?}");
                }
            }
        })
    }

    fn start_write_task(
        mut server_recv: NTServerReceiver,
        mut write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    ) -> JoinHandle<Result<(), SendMessageError>> {
        tokio::spawn(async move {
            while let Ok(message) = server_recv.recv().await {
                let packet = match &*message {
                    ServerboundMessage::Text(json) => serde_json::to_string(&[json]).map_err(|err| err.into()).map(Message::Text),
                    ServerboundMessage::Binary(binary) => rmp_serde::to_vec(binary).map_err(|err| err.into()).map(Message::Binary),
                    ServerboundMessage::Ping => Ok(Message::Ping(Vec::new())),
                };
                match packet {
                    Ok(packet) => {
                        if !matches!(packet, Message::Ping(_)) { debug!("sent message: {packet:?}"); };
                        if (write.send(packet).await).is_err() { return Err(SendMessageError::ConnectionClosed(ConnectionClosedError)); };
                    },
                    Err(err) => return Err(err),
                };
            };
            Ok(())
        })
    }

    fn start_read_task(
        read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        update_time_sender: mpsc::Sender<(Duration, Duration)>,
        pong_send: Arc<Notify>,
        announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
        client_sender: NTClientSender,
    ) -> JoinHandle<Result<(), ReceiveMessageError>> {
        tokio::spawn(async move {
            read.err_into().try_for_each(|message| async {
                let message = match message {
                    Message::Binary(binary) => {
                        let mut binary = VecDeque::from(binary);
                        let mut binary_data = Vec::new();
                        while !binary.is_empty() {
                            let binary = rmp_serde::from_read::<&mut VecDeque<u8>, BinaryData>(&mut binary)?;
                            if binary.id == -1 {
                                let client_send_time = Duration::from_micros(binary.data.as_u64().expect("timestamp data is u64"));
                                if update_time_sender.send((binary.timestamp, client_send_time)).await.is_err() {
                                    return Err(ReceiveMessageError::ConnectionClosed(ConnectionClosedError));
                                };
                            }
                            binary_data.push(ClientboundData::Binary(binary));
                        };
                        Some(binary_data)
                    },
                    Message::Text(json) => {
                        Some(serde_json::from_str::<'_, Vec<ClientboundTextData>>(&json)?.into_iter().map(ClientboundData::Text).collect())
                    },
                    Message::Pong(_) => {
                        pong_send.notify_one();
                        None
                    },
                    Message::Close(_) => return Err(ReceiveMessageError::ConnectionClosed(ConnectionClosedError)),
                    _ => None,
                };

                if let Some(data_frame) = message {
                    debug!("received message(s): {data_frame:?}");
                    for data in data_frame {
                        match &data {
                            ClientboundData::Text(ClientboundTextData::Announce(announce)) => {
                                let mut announced_topics = announced_topics.write().await;
                                announced_topics.insert(announce.id, announce.into());
                            },
                            ClientboundData::Text(ClientboundTextData::Unannounce(Unannounce { id, .. })) => {
                                let mut announced_topics = announced_topics.write().await;
                                announced_topics.remove(id);
                            },
                            // TODO: handle Properties
                            _ => {},
                        }

                        client_sender.send(data.into()).map_err(|_| ConnectionClosedError)?;
                    }
                };

                Ok(())
            }).await
        })
    }
}

/// Options when creating a new [`Client`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NewClientOptions {
    /// The address to connect to.
    /// 
    /// Default is [`NTAddr::Local`].
    pub addr: NTAddr,
    /// The port of the server.
    ///
    /// Default is `5810`.
    pub unsecure_port: u16,
    /// The port of the server.
    ///
    /// Default is 5811`.
    pub secure_port: u16,
    /// The name of the client.
    ///
    /// Default is `rust-client-{random u16}`
    pub name: String,
    /// The timeout for a server response.
    ///
    /// If this timeout gets exceeded when a server response is expected, such as in PING requests,
    /// the client will close the connection and attempt to reconnect.
    ///
    /// Default is 1s.
    pub response_timeout: Duration,
    /// The interval at which to send ping messages.
    ///
    /// Default is 200ms.
    pub ping_interval: Duration,
    /// The interval at which to update server time.
    ///
    /// Default is 5s.
    pub update_time_interval: Duration,
}

impl Default for NewClientOptions {
    fn default() -> Self {
        Self {
            addr: Default::default(),
            unsecure_port: 5810,
            secure_port: 5811,
            name: format!("rust-client-{}", rand::random::<u16>()),
            response_timeout: Duration::from_secs(1),
            ping_interval: Duration::from_millis(200),
            update_time_interval: Duration::from_secs(5),
        }
    }
}

/// Represents an address that a `NetworkTables` client can connect to.
///
/// By default, this is set to [`NTAddr::Local`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NTAddr {
    /// Address corresponding to an FRC team number.
    ///
    /// This should be used when deploying code to the robot.
    ///
    /// IP addresses are in the format of `10.TE.AM.2`.
    /// - Team 1: `10.0.1.2`
    /// - Team 12: `10.0.12.2`
    /// - Team 12345: `10.123.45.2`
    /// - Team 5071: `10.50.71.2`
    TeamNumber(u16),
    /// Local address (`127.0.0.1`).
    ///
    /// This should be used while in robot simulation mode.
    Local,
    /// Custom address.
    ///
    /// This is useful when the server is running simulate on a separate machine.
    Custom(Ipv4Addr),
}

impl Default for NTAddr {
    fn default() -> Self {
        Self::Local
    }
}

impl NTAddr {
    /// Converts this into an [`Ipv4Addr`].
    pub fn into_addr(self) -> Result<Ipv4Addr, IntoAddrError> {
        let addr = match self {
            NTAddr::TeamNumber(team_number) => {
                if team_number > 25599 { return Err(IntoAddrError::InvalidTeamNumber(team_number)); };
                let first_section = team_number / 100;
                let last_two = team_number % 100;
                Ipv4Addr::new(10, first_section.try_into().unwrap(), last_two.try_into().unwrap(), 2)
            },
            NTAddr::Local => Ipv4Addr::LOCALHOST,
            NTAddr::Custom(addr) => addr,
        };
        Ok(addr)
    }
}

/// Time information about a `NetworkTables` server and client.
///
/// Provides methods to retrieve both the client's internal time and the server's time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetworkTablesTime {
    started: Instant,
    offset: time::Duration,
}

impl Default for NetworkTablesTime {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkTablesTime {
    /// Creates a new `NetworkTablesTime` with the client start time of [`Instant::now`] and a
    /// server offset time of [`Duration::ZERO`].
    pub fn new() -> Self {
        Self { started: Instant::now(), offset: time::Duration::ZERO }
    }

    /// Returns the current client time.
    pub fn client_time(&self) -> time::Duration {
        Instant::now().signed_duration_since(self.started)
    }

    /// Returns the current server time.
    ///
    /// # Panics
    /// Panics if the calculated server time is negative. This should never happen and be reported
    /// if it does.
    pub fn server_time(&self) -> Duration {
        match (self.client_time() + self.offset).try_into() {
            Ok(duration) => duration,
            Err(_) => panic!("expected server time to be positive"),
        }
    }
}

/// Continuously calls `init` whenever it returns an error, effectively becoming a reconnect
/// handler.
///
/// This function will only ever return if an [`Ok`][`Result::Ok`] value or a
/// [`std::io::Error`] error is returned, returning that result.
///
/// # Examples
/// ```no_run
/// use nt_client::{subscribe::ReceivedMessage, Client};
///
/// # tokio_test::block_on(async {
/// nt_client::reconnect(|| async {
///     let mut client = Client::new(Default::default());
///
///     let topic = client.topic("/topic");
///     let sub_task = tokio::spawn(async move {
///         let mut subscriber = topic.subscribe(Default::default()).await;
///
///         while let Ok(ReceivedMessage::Updated((_, value))) = subscriber.recv().await {
///             println!("updated: {value:?}");
///         }
///     });
///
///     // select! to make sure other tasks don't stay running
///     tokio::select! {
///         res = client.connect() => res,
///         _ = sub_task => Ok(()),
///     }
/// }).await.unwrap();
/// # })
/// ```
pub async fn reconnect<F, I>(mut init: I) -> Result<(), std::io::Error>
where
    F: Future<Output = Result<(), ConnectError>>,
    I: FnMut() -> F,
{
    loop {
        match init().await {
            Ok(_) => return Ok(()),
            Err(ConnectError::WebsocketError(tungstenite::Error::Io(err))) => {
                error!("fatal error occurred: {err}");
                return Err(err);
            },
            Err(err) => {
                error!("client crashed! {err}");
                info!("attempting to reconnect");
            },
        }
    }
}

pub(crate) async fn recv_until<T, F>(recv_ws: &mut NTClientReceiver, mut filter: F) -> Result<T, broadcast::error::RecvError>
where F: FnMut(Arc<ClientboundData>) -> Option<T>
{
    loop {
        if let Some(data) = filter(recv_ws.recv().await?) {
            return Ok(data);
        }
    };
}

pub(crate) async fn recv_until_async<T, F, Fu>(recv_ws: &mut NTClientReceiver, mut filter: F) -> Result<T, broadcast::error::RecvError>
where
    Fu: Future<Output = Option<T>>,
    F: FnMut(Arc<ClientboundData>) -> Fu,
{
    loop {
        if let Some(data) = filter(recv_ws.recv().await?).await {
            return Ok(data);
        }
    };
}

