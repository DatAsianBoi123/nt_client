// TODO: use tracing instead of println!
// TODO: use less `expect` and `unwrap`!!
// TODO: documentation
// TODO: add derives to pub structs (Debug, Clone, Eq, etc.)

// #![warn(missing_docs)]

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
//! use nt_client::{Client, NewClientOptions, NTAddr};
//!
//! # tokio_test::block_on(async {
//! let options = NewClientOptions { addr: NTAddr::Local, ..Default::default() };
//! let client = Client::new(options);
//!
//! let thing_topic = client.topic("/thing");
//! tokio::spawn(async move {
//!     let mut sub = thing_topic.subscribe::<String>(Default::default())
//!         .await
//!         .expect("websocket connection closed!");
//!
//!     while let Ok(recv) = sub.recv().await {
//!         println!("topic updated: '{recv}'")
//!     }
//! });
//! 
//! client.connect().await.unwrap();
//!
//! # });
//! ```
//! 
//! [NetworkTables]: https://github.com/wpilibsuite/allwpilib/blob/main/ntcore/doc/networktables4.adoc

use core::panic;
use std::{collections::VecDeque, convert::Into, net::{Ipv4Addr, SocketAddrV4}, sync::Arc, time::{Duration, Instant}};

use dataframe::{BinaryData, ClientboundData, ClientboundDataFrame, ServerboundMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::{select, sync::{broadcast, mpsc, Notify, RwLock}, time::{interval, sleep, timeout}};
use tokio_tungstenite::tungstenite::{self, Message};
use topic::Topic;

pub mod error;
// TODO: rename to `data`
pub mod dataframe;
pub mod topic;
pub mod subscribe;
pub mod publish;

pub type NTServerSender = broadcast::Sender<Arc<ServerboundMessage>>;
pub type NTServerReceiver = broadcast::Receiver<Arc<ServerboundMessage>>;

pub type NTClientSender = broadcast::Sender<Arc<ClientboundData>>;
pub type NTClientReceiver = broadcast::Receiver<Arc<ClientboundData>>;

/// The client used to interact with a `NetworkTables` server.
///
/// When this goes out of scope, the websocket connection is closed and no attempts to reconnect
/// will be made.
pub struct Client {
    addr: SocketAddrV4,
    name: String,
    time: Arc<RwLock<NetworkTablesTime>>,

    send_ws: (NTServerSender, NTServerReceiver),
    recv_ws: (NTClientSender, NTClientReceiver),
}

impl Client {
    pub const PING_INTERVAL: Duration = Duration::from_millis(200);
    pub const PONG_TIMEOUT: Duration = Duration::from_secs(1);

    pub const UPDATE_TIME_INTERVAL: Duration = Duration::from_secs(5);

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

    /// Returns a new topic with a given name.
    pub fn topic(&self, name: impl ToString) -> Topic {
        Topic::new(name.to_string(), self.time(), self.send_ws.0.clone(), self.recv_ws.0.clone())
    }

    /// Connects to the `NetworkTables` server.
    ///
    /// This future will only complete when the client has disconnected from the server.
    pub async fn connect(mut self) -> Result<(), ConnectError> {
        // TODO: try connecting to wss first
        let uri = format!("ws://{}/nt/{}", self.addr, self.name);
        let (ws_stream, _) = tokio_tungstenite::connect_async(uri).await?;
        println!("connected to websocket");

        let (mut write, read) = ws_stream.split();

        let ping_task_ws_sender = self.send_ws.0.clone();
        let pong_notify_recv = Arc::new(Notify::new());
        let pong_notify_send = pong_notify_recv.clone();
        // TODO: split tasks into their own methods
        let interval_ping = tokio::spawn(async move {
            sleep(Self::PING_INTERVAL).await;

            let mut interval = interval(Self::PING_INTERVAL);
            loop {
                interval.tick().await;
                ping_task_ws_sender.send(ServerboundMessage::Ping.into()).expect("receiver still exists");

                if (timeout(Self::PONG_TIMEOUT, pong_notify_recv.notified()).await).is_err() {
                    // TODO: reconnect instead of panicking
                    panic!("pong not received within 1 second! terminating connection");
                }
            }
        });

        let (update_time_sender, mut update_time_recv) = mpsc::channel(1);
        let update_time_ws = self.send_ws.0.clone();
        let update_time = tokio::spawn(async move {
            let mut interval = interval(Self::UPDATE_TIME_INTERVAL);
            loop {
                interval.tick().await;

                let client_time = {
                    let time = self.time.read().await;
                    time.client_time()
                };
                // TODO: handle client time overflow
                let data = BinaryData::new::<u64>(-1, Duration::ZERO, client_time.as_micros().try_into().expect("client time overflowed"));
                update_time_ws.send(ServerboundMessage::Binary(data).into()).expect("receiver still exists");

                if let Some((timestamp, client_send_time)) = update_time_recv.recv().await {
                    let offset = {
                        let now = self.time.read().await.client_time();
                        let rtt = now - client_send_time;
                        let server_time = timestamp - rtt / 2;

                        server_time - now
                    };

                    let mut time = self.time.write().await;
                    time.offset = offset;
                    println!("updated time");
                }
            }
        });

        let write_task = tokio::spawn(async move {
            while let Ok(message) = self.send_ws.1.recv().await {
                let packet = match &*message {
                    ServerboundMessage::Text(json) => Message::Text(serde_json::to_string(&[json]).expect("can serialize to json")),
                    ServerboundMessage::Binary(binary) => Message::Binary(rmp_serde::to_vec(binary).expect("can serialize to binary")),
                    ServerboundMessage::Ping => Message::Ping(Vec::new()),
                };
                if !matches!(packet, Message::Ping(_)) { println!("sent message: {packet:?}"); };
                write.send(packet).await.expect("can send message");
            }
        });

        let read_task = tokio::spawn(async move {
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
                        pong_notify_send.notify_one();
                        None
                    },
                    // TODO: reconnect instead of panicking
                    Message::Close(_) => panic!("websocket closed"),
                    _ => None,
                };

                if let Some(data_frame) = message {
                    println!("received message(s): {data_frame:?}");
                    for data in data_frame {
                        self.recv_ws.0.send(data.into()).expect("receivers exist");
                    }
                };
            }).await;
        });

        // TODO: actual error handling here
        select! {
            _ = interval_ping => eprintln!("interval pinger stopped!"),
            _ = write_task => eprintln!("write task stopped!"),
            _ = read_task => eprintln!("read task stopped!"),
            _ = update_time => eprintln!("update time stopped!"),
        };
        println!("closing connection");
        Ok(())
    }
}

/// Options when creating a new [`Client`].
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
}

impl Default for NewClientOptions {
    fn default() -> Self {
        Self { addr: Default::default(), port: 5810, name: format!("rust-client-{}", rand::random::<u16>()) }
    }
}

/// Represents an address that a `NetworkTables` client can connect to.
///
/// By default, this is set to [`NTAddr::Local`].
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

