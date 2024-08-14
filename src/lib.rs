// TODO: use less `expect` and `unwrap`!!
// TODO: inconsistencies with Gets and Returns in docs

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
use std::{collections::{HashMap, VecDeque}, convert::Into, net::{Ipv4Addr, SocketAddrV4}, sync::Arc, time::{Duration, Instant}};

use data::{BinaryData, ClientboundData, ClientboundDataFrame, ClientboundTextData, ServerboundMessage, Unannounce};
use futures_util::{stream::{SplitSink, SplitStream}, Future, SinkExt, StreamExt};
use tokio::{net::TcpStream, select, sync::{broadcast, mpsc, Notify, RwLock}, task::JoinHandle, time::{interval, timeout}};
use tokio_tungstenite::{tungstenite::{self, Message}, MaybeTlsStream, WebSocketStream};
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
    addr: SocketAddrV4,
    name: String,
    time: Arc<RwLock<NetworkTablesTime>>,
    announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,

    response_timeout: Duration,
    ping_interval: Duration,
    update_time_interval: Duration,

    send_ws: (NTServerSender, NTServerReceiver),
    recv_ws: (NTClientSender, NTClientReceiver),
}

impl Client {
    /// Creates a new `Client` with options.
    ///
    /// # Panics
    /// Panics if the [`NTAddr::TeamNumber`] team number is greater than 25599.
    pub fn new(options: NewClientOptions) -> Self {
        let addr = if let Some(addr) = options.addr.into_addr() { addr } else { panic!("team_number is greater than 25599"); };

        Client {
            addr: SocketAddrV4::new(addr, options.port),
            name: options.name,
            time: Default::default(),
            announced_topics: Default::default(),

            response_timeout: options.response_timeout,
            ping_interval: options.ping_interval,
            update_time_interval: options.update_time_interval,

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
        Topic::new(name.to_string(), self.time(), self.announced_topics.clone(), self.response_timeout, self.send_ws.0.clone(), self.recv_ws.0.clone())
    }

    /// Connects to the `NetworkTables` server.
    ///
    /// This future will only complete when the client has disconnected from the server.
    pub async fn connect(self) -> Result<(), ConnectError> {
        // TODO: try connecting to wss first
        let uri = format!("ws://{}/nt/{}", self.addr, self.name);
        let (ws_stream, _) = tokio_tungstenite::connect_async(uri.clone()).await?;
        info!("connected to server at {uri}");

        let (write, read) = ws_stream.split();

        let pong_notify_recv = Arc::new(Notify::new());
        let pong_notify_send = pong_notify_recv.clone();
        let ping_task = Client::start_ping_task(pong_notify_recv, self.send_ws.0.clone(), self.ping_interval, self.response_timeout);

        let (update_time_sender, update_time_recv) = mpsc::channel(1);
        let update_time_task = Client::start_update_time_task(self.update_time_interval, self.time(), self.send_ws.0.clone(), update_time_recv);

        let announced_topics = self.announced_topics();
        let write_task = Client::start_write_task(self.send_ws.1, write);
        let read_task = Client::start_read_task(read, update_time_sender, pong_notify_send, announced_topics, self.recv_ws.0);

        // TODO: actual error handling here
        select! {
            _ = ping_task => error!("interval pinger stopped!"),
            _ = write_task => error!("write task stopped!"),
            _ = read_task => error!("read task stopped!"),
            _ = update_time_task => error!("update time stopped!"),
        };
        info!("closing connection");
        Ok(())
    }

    fn start_ping_task(pong_recv: Arc<Notify>, ws_sender: NTServerSender, ping_interval: Duration, response_timeout: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = interval(ping_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                ws_sender.send(ServerboundMessage::Ping.into()).expect("receiver still exists");

                if (timeout(response_timeout, pong_recv.notified()).await).is_err() {
                    // TODO: reconnect instead of panicking
                    panic!("pong not received within 1 second! terminating connection");
                }
            }
        })
    }

    fn start_update_time_task(
        update_time_interval: Duration,
        time: Arc<RwLock<NetworkTablesTime>>,
        ws_sender: NTServerSender,
        mut time_recv: mpsc::Receiver<(Duration, Duration)>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = interval(update_time_interval);
            loop {
                interval.tick().await;

                let client_time = {
                    let time = time.read().await;
                    time.client_time()
                };
                // TODO: handle client time overflow
                let data = BinaryData::new::<u64>(-1, Duration::ZERO, client_time.as_micros().try_into().expect("client time overflowed"));
                ws_sender.send(ServerboundMessage::Binary(data).into()).expect("receiver still exists");

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
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok(message) = server_recv.recv().await {
                let packet = match &*message {
                    ServerboundMessage::Text(json) => Message::Text(serde_json::to_string(&[json]).expect("can serialize to json")),
                    ServerboundMessage::Binary(binary) => Message::Binary(rmp_serde::to_vec(binary).expect("can serialize to binary")),
                    ServerboundMessage::Ping => Message::Ping(Vec::new()),
                };
                if !matches!(packet, Message::Ping(_)) { debug!("sent message: {packet:?}"); };
                write.send(packet).await.expect("can send message");
            }
        })
    }

    fn start_read_task(
        read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        update_time_sender: mpsc::Sender<(Duration, Duration)>,
        pong_send: Arc<Notify>,
        announced_topics: Arc<RwLock<HashMap<i32, AnnouncedTopic>>>,
        client_sender: NTClientSender,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            read.for_each(|message| async {
                let message = message.expect("can read message");

                let message = match message {
                    Message::Binary(binary) => {
                        let mut binary = VecDeque::from(binary);
                        let mut binary_data = Vec::new();
                        while let Ok(binary) = rmp_serde::from_read::<&mut VecDeque<u8>, BinaryData>(&mut binary) {
                            if binary.id == -1 {
                                let client_send_time = Duration::from_micros(binary.data.as_u64().expect("timestamp data is u64"));
                                update_time_sender.send((binary.timestamp, client_send_time)).await.expect("receiver exists");
                            }

                            binary_data.push(binary);
                        };
                        Some(ClientboundDataFrame::Binary(binary_data))
                    },
                    Message::Text(json) => {
                        let json = serde_json::from_str(&json).expect("can deserialize to json");
                        Some(ClientboundDataFrame::Text(json))
                    },
                    Message::Pong(_) => {
                        pong_send.notify_one();
                        None
                    },
                    // TODO: reconnect instead of panicking
                    Message::Close(_) => panic!("websocket closed"),
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
                            _ => (),
                        }

                        client_sender.send(data.into()).expect("receivers exist");
                    }
                };
            }).await;
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
    /// Default is `5810` for unsecure connections and `5811` for secure ones.
    pub port: u16,
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
            port: 5810,
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
    // NOTE: return Result instead of Option?
    pub fn into_addr(self) -> Option<Ipv4Addr> {
        let addr = match self {
            NTAddr::TeamNumber(team_number) => {
                if team_number > 25599 { return None; };
                let first_section = team_number / 100;
                let last_two = team_number % 100;
                Ipv4Addr::new(10, first_section.try_into().unwrap(), last_two.try_into().unwrap(), 2)
            },
            NTAddr::Local => Ipv4Addr::LOCALHOST,
            NTAddr::Custom(addr) => addr,
        };
        Some(addr)
    }
}

/// Errors than can occur when connecting to a `NetworkTables` server.
#[derive(thiserror::Error, Debug)]
pub enum ConnectError {
    /// An error occurred with the websocket.
    #[error("websocket error: {0}")]
    WebsocketError(#[from] tungstenite::Error),
}

/// Time information about a `NetworkTables` server and client.
///
/// Provides methods to retrieve both the client's internal time and the server's time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetworkTablesTime {
    started: Instant,
    offset: Duration,
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
        Self { started: Instant::now(), offset: Duration::ZERO }
    }

    /// Returns the current client time.
    pub fn client_time(&self) -> Duration {
        Instant::now().duration_since(self.started)
    }

    /// Returns the current server time.
    pub fn server_time(&self) -> Duration {
        self.client_time() + self.offset
    }
}

// TODO: custom `RecvError` struct instead of using tokio one?
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

