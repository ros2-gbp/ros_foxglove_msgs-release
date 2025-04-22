use crate::channel::ChannelId;
use crate::cow_vec::CowVec;
use crate::websocket::protocol::client::Subscription;
pub use crate::websocket::protocol::client::{
    ClientChannel, ClientChannelId, ClientMessage, SubscriptionId,
};
pub use crate::websocket::protocol::server::Capability;
#[cfg(feature = "unstable")]
pub use crate::websocket::protocol::server::{Parameter, ParameterType, ParameterValue};
use crate::{get_runtime_handle, Channel, FoxgloveError, LogSink, Metadata};
use bimap::BiHashMap;
use bytes::{BufMut, BytesMut};
use flume::TrySendError;
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use std::collections::hash_map::Entry;
use std::collections::HashSet;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Weak;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tokio_tungstenite::{
    tungstenite::{self, handshake::server, http::HeaderValue, Message},
    WebSocketStream,
};
use tokio_util::sync::CancellationToken;

mod protocol;
#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "unstable"))]
mod unstable_tests;

/// Identifies a client connection. Unique for the duration of the server's lifetime.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClientId(u32);

/// A connected client session with the websocket server.
#[derive(Debug)]
pub struct Client<'a>(&'a ConnectedClient);

impl Client<'_> {
    /// Returns the client ID.
    pub fn id(&self) -> ClientId {
        self.0.id
    }
}

/// Information about a client channel.
#[derive(Debug)]
pub struct ClientChannelView<'a> {
    id: ClientChannelId,
    topic: &'a str,
}

impl ClientChannelView<'_> {
    /// Returns the client channel ID.
    pub fn id(&self) -> ClientChannelId {
        self.id
    }

    /// Returns the topic of the client channel.
    pub fn topic(&self) -> &str {
        self.topic
    }
}

/// Information about a channel.
#[derive(Debug)]
pub struct ChannelView<'a> {
    id: ChannelId,
    topic: &'a str,
}

impl ChannelView<'_> {
    /// Returns the channel ID.
    pub fn id(&self) -> ChannelId {
        self.id
    }

    /// Returns the topic of the channel.
    pub fn topic(&self) -> &str {
        self.topic
    }
}

pub(crate) const SUBPROTOCOL: &str = "foxglove.sdk.v1";

type WebsocketSender = SplitSink<WebSocketStream<TcpStream>, Message>;

// Queue up to 1024 messages per connected client before dropping messages
const DEFAULT_MESSAGE_BACKLOG_SIZE: usize = 1024;
const DEFAULT_CONTROL_PLANE_BACKLOG_SIZE: usize = 64;

#[derive(Error, Debug)]
enum WSError {
    #[error("client handshake failed")]
    HandshakeError,
}

#[derive(Default)]
pub(crate) struct ServerOptions {
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub message_backlog_size: Option<usize>,
    pub listener: Option<Arc<dyn ServerListener>>,
    pub capabilities: Option<HashSet<Capability>>,
    pub supported_encodings: Option<HashSet<String>>,
    pub runtime: Option<Handle>,
}

impl std::fmt::Debug for ServerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerOptions")
            .field("session_id", &self.session_id)
            .field("name", &self.name)
            .field("message_backlog_size", &self.message_backlog_size)
            .finish()
    }
}

/// A websocket server that implements the Foxglove WebSocket Protocol
pub(crate) struct Server {
    /// A weak reference to the Arc holding the server.
    /// This is used to get a reference to the outer `Arc<Server>` from Server methods.
    /// See the arc() method and its callers. We need the Arc so we can use it in async futures
    /// which need to prove to the compiler that the server will outlive the future.
    /// It's analogous to the mixin shared_from_this in C++.
    weak_self: Weak<Self>,
    started: AtomicBool,
    message_backlog_size: u32,
    runtime: Handle,
    /// May be provided by the caller
    session_id: String,
    name: String,
    clients: CowVec<Arc<ConnectedClient>>,
    channels: parking_lot::RwLock<HashMap<ChannelId, Arc<Channel>>>,
    /// Callbacks for handling client messages, etc.
    listener: Option<Arc<dyn ServerListener>>,
    /// Capabilities advertised to clients
    capabilities: HashSet<Capability>,
    /// Encodings server can accept from clients. Ignored unless the "clientPublish" capability is set.
    supported_encodings: HashSet<String>,
    /// Token for cancelling all tasks
    cancellation_token: CancellationToken,
}

/// Provides a mechanism for registering callbacks for
/// handling client message events.
pub trait ServerListener: Send + Sync {
    /// Callback invoked when a client message is received.
    fn on_message_data(
        &self,
        _client: Client,
        _client_channel: ClientChannelView,
        _payload: &[u8],
    ) {
    }
    /// Callback invoked when a client subscribes to a channel.
    /// Only invoked if the channel is associated with the server and isn't already subscribed to by the client.
    fn on_subscribe(&self, _client: Client, _channel: ChannelView) {}
    /// Callback invoked when a client unsubscribes from a channel.
    /// Only invoked for channels that had an active subscription from the client.
    fn on_unsubscribe(&self, _client: Client, _channel: ChannelView) {}
    /// Callback invoked when a client advertises a client channel. Requires the "clientPublish" capability.
    fn on_client_advertise(&self, _client: Client, _channel: ClientChannelView) {}
    /// Callback invoked when a client unadvertises a client channel. Requires the "clientPublish" capability.
    fn on_client_unadvertise(&self, _client: Client, _channel: ClientChannelView) {}
}

/// A connected client session with the websocket server.
struct ConnectedClient {
    id: ClientId,
    addr: SocketAddr,
    /// Write side of a WS stream
    sender: Mutex<WebsocketSender>,
    data_plane_tx: flume::Sender<Message>,
    data_plane_rx: flume::Receiver<Message>,
    control_plane_tx: flume::Sender<Message>,
    control_plane_rx: flume::Receiver<Message>,
    /// Subscriptions from this client
    subscriptions: parking_lot::Mutex<BiHashMap<ChannelId, SubscriptionId>>,
    /// Channels advertised by this client
    advertised_channels: parking_lot::Mutex<HashMap<ClientChannelId, Arc<ClientChannel>>>,
    /// Optional callback handler for a server implementation
    server_listener: Option<Arc<dyn ServerListener>>,
    server: Weak<Server>,
}

impl ConnectedClient {
    /// Handle a text or binary message sent from the client.
    ///
    /// Standard protocol messages (such as Close) should be handled upstream.
    fn handle_message(&self, message: Message) {
        if message.is_binary() {
            self.handle_binary_message(message);
        } else {
            self.handle_text_message(message);
        }
    }

    fn handle_text_message(&self, message: Message) {
        let Ok(message) = message.to_text() else {
            tracing::error!("Received invalid message from {}", self.addr);
            self.send_error(format!("Invalid message: {message}"));
            return;
        };
        let Some(server) = self.server.upgrade() else {
            tracing::error!("Server closed");
            return;
        };

        match serde_json::from_str::<ClientMessage>(message) {
            Ok(ClientMessage::Subscribe { subscriptions }) => {
                self.on_subscribe(server, subscriptions);
            }
            Ok(ClientMessage::Unsubscribe { subscription_ids }) => {
                self.on_unsubscribe(server, subscription_ids);
            }
            Ok(ClientMessage::Advertise { channels }) => {
                self.on_advertise(server, channels);
            }
            Ok(ClientMessage::Unadvertise { channel_ids }) => {
                self.on_unadvertise(channel_ids);
            }
            _ => {
                tracing::error!("Unsupported message from {}: {message}", self.addr);
                self.send_error(format!("Unsupported message: {message}"));
            }
        }
    }

    fn handle_binary_message(&self, message: Message) {
        if message.is_empty() {
            tracing::debug!("Received empty binary message from {}", self.addr);
            return;
        }

        let msg_bytes = message.into_data();
        let opcode = protocol::client::BinaryOpcode::from_repr(msg_bytes[0]);
        match opcode {
            Some(protocol::client::BinaryOpcode::MessageData) => {
                match protocol::client::parse_binary_message(&msg_bytes) {
                    Ok((channel_id, payload)) => {
                        let client_channel = {
                            let advertised_channels = self.advertised_channels.lock();
                            let Some(channel) = advertised_channels.get(&channel_id) else {
                                tracing::error!(
                                    "Received message for unknown channel: {}",
                                    channel_id
                                );
                                self.send_error(format!("Unknown channel ID: {}", channel_id));
                                // Do not forward to server listener
                                return;
                            };
                            channel.clone()
                        };
                        // Call the handler after releasing the advertised_channels lock
                        if let Some(handler) = self.server_listener.as_ref() {
                            handler.on_message_data(
                                Client(self),
                                ClientChannelView {
                                    id: client_channel.id,
                                    topic: &client_channel.topic,
                                },
                                payload,
                            );
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to parse binary message: {err}");
                        self.send_error(format!("Failed to parse binary message: {err}"));
                    }
                }
            }
            Some(_) => {
                tracing::error!("Opcode not yet implemented: {}", msg_bytes[0]);
            }
            None => {
                tracing::error!("Invalid binary opcode: {}", msg_bytes[0]);
                self.send_error(format!("Invalid binary opcode: {}", msg_bytes[0]));
            }
        }
    }

    fn on_unadvertise(&self, mut channel_ids: Vec<ClientChannelId>) {
        let mut client_channels = Vec::with_capacity(channel_ids.len());
        // Using a limited scope and iterating twice to avoid holding the lock on advertised_channels while calling on_client_unadvertise
        {
            let mut advertised_channels = self.advertised_channels.lock();
            let mut i = 0;
            while i < channel_ids.len() {
                let id = channel_ids[i];
                let Some(channel) = advertised_channels.remove(&id) else {
                    // Remove the channel ID from the list so we don't invoke the on_client_unadvertise callback
                    channel_ids.swap_remove(i);
                    self.send_warning(format!(
                        "Client is not advertising channel: {}; ignoring unadvertisement",
                        id
                    ));
                    continue;
                };
                client_channels.push(channel.clone());
                i += 1;
            }
        }
        // Call the handler after releasing the advertised_channels lock
        if let Some(handler) = self.server_listener.as_ref() {
            for (id, client_channel) in channel_ids.iter().cloned().zip(client_channels) {
                handler.on_client_unadvertise(
                    Client(self),
                    ClientChannelView {
                        id,
                        topic: &client_channel.topic,
                    },
                );
            }
        }
    }

    fn on_advertise(&self, server: Arc<Server>, channels: Vec<ClientChannel>) {
        if !server.capabilities.contains(&Capability::ClientPublish) {
            self.send_error("Server does not support clientPublish capability".to_string());
            return;
        }

        for channel in channels {
            // Using a limited scope here to avoid holding the lock on advertised_channels while calling on_client_advertise
            let client_channel = {
                match self.advertised_channels.lock().entry(channel.id) {
                    Entry::Occupied(_) => {
                        self.send_warning(format!(
                            "Client is already advertising channel: {}; ignoring advertisement",
                            channel.id
                        ));
                        continue;
                    }
                    Entry::Vacant(entry) => {
                        let client_channel = Arc::new(channel);
                        entry.insert(client_channel.clone());
                        client_channel
                    }
                }
            };

            // Call the handler after releasing the advertised_channels lock
            if let Some(handler) = self.server_listener.as_ref() {
                handler.on_client_advertise(
                    Client(self),
                    ClientChannelView {
                        id: client_channel.id,
                        topic: &client_channel.topic,
                    },
                );
            }
        }
    }

    fn on_unsubscribe(&self, server: Arc<Server>, subscription_ids: Vec<SubscriptionId>) {
        let mut unsubscribed_channel_ids = Vec::with_capacity(subscription_ids.len());
        // First gather the unsubscribed channel ids while holding the subscriptions lock
        {
            let mut subscriptions = self.subscriptions.lock();
            for subscription_id in subscription_ids {
                if let Some((channel_id, _)) = subscriptions.remove_by_right(&subscription_id) {
                    unsubscribed_channel_ids.push(channel_id);
                }
            }
        }

        // If we don't have a ServerListener, we're done.
        let Some(handler) = self.server_listener.as_ref() else {
            return;
        };

        // Then gather the actual channel references while holding the channels lock
        let mut unsubscribed_channels = Vec::with_capacity(unsubscribed_channel_ids.len());
        {
            let channels = server.channels.read();
            for channel_id in unsubscribed_channel_ids {
                if let Some(channel) = channels.get(&channel_id) {
                    unsubscribed_channels.push(channel.clone());
                }
            }
        }

        // Finally call the handler for each channel
        for channel in unsubscribed_channels {
            handler.on_unsubscribe(
                Client(self),
                ChannelView {
                    id: channel.id,
                    topic: &channel.topic,
                },
            );
        }
    }

    fn on_subscribe(&self, server: Arc<Server>, mut subscriptions: Vec<Subscription>) {
        // First prune out any subscriptions for channels not in the channel map,
        // limiting how long we need to hold the lock.
        let mut subscribed_channels = Vec::with_capacity(subscriptions.len());
        {
            let channels = server.channels.read();
            let mut i = 0;
            while i < subscriptions.len() {
                let subscription = &subscriptions[i];
                let Some(channel) = channels.get(&subscription.channel_id) else {
                    tracing::error!(
                        "Client {} attempted to subscribe to unknown channel: {}",
                        self.addr,
                        subscription.channel_id
                    );
                    self.send_error(format!("Unknown channel ID: {}", subscription.channel_id));
                    // Remove the subscription from the list so we don't invoke the on_subscribe callback for it
                    subscriptions.swap_remove(i);
                    continue;
                };
                subscribed_channels.push(channel.clone());
                i += 1
            }
        }

        for (subscription, channel) in subscriptions.into_iter().zip(subscribed_channels) {
            // Using a limited scope here to avoid holding the lock on subscriptions while calling on_subscribe
            {
                let mut subscriptions = self.subscriptions.lock();
                if subscriptions
                    .insert_no_overwrite(subscription.channel_id, subscription.id)
                    .is_err()
                {
                    if subscriptions.contains_left(&subscription.channel_id) {
                        self.send_warning(format!(
                            "Client is already subscribed to channel: {}; ignoring subscription",
                            subscription.channel_id
                        ));
                    } else {
                        assert!(subscriptions.contains_right(&subscription.id));
                        self.send_error(format!(
                            "Subscription ID was already used: {}; ignoring subscription",
                            subscription.id
                        ));
                    }
                    continue;
                }
            }

            tracing::info!(
                "Client {} subscribed to channel {} with subscription id {}",
                self.addr,
                subscription.channel_id,
                subscription.id
            );
            if let Some(handler) = self.server_listener.as_ref() {
                handler.on_subscribe(
                    Client(self),
                    ChannelView {
                        id: channel.id,
                        topic: &channel.topic,
                    },
                );
            }
        }
    }

    /// Send an ad hoc error status message to the client, with the given message.
    fn send_error(&self, message: String) {
        self.send_status(protocol::server::StatusLevel::Error, message, None);
    }

    /// Send an ad hoc warning status message to the client, with the given message.
    fn send_warning(&self, message: String) {
        self.send_status(protocol::server::StatusLevel::Warning, message, None);
    }

    fn send_status(
        &self,
        level: protocol::server::StatusLevel,
        message: String,
        id: Option<String>,
    ) {
        let status = protocol::server::Status { level, message, id };
        let message = Message::text(serde_json::to_string(&status).unwrap());
        // If the message can't be sent, or the outbox is full, log a warning and continue.
        self.data_plane_tx.try_send(message).unwrap_or_else(|err| {
            tracing::warn!("Failed to send status to client {}: {err}", self.addr)
        });
    }
}

impl std::fmt::Debug for ConnectedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id)
            .field("address", &self.addr)
            .finish()
    }
}

// A websocket server that implements the Foxglove WebSocket Protocol
impl Server {
    pub fn new(weak_self: Weak<Self>, opts: ServerOptions) -> Self {
        Server {
            weak_self,
            started: AtomicBool::new(false),
            message_backlog_size: opts
                .message_backlog_size
                .unwrap_or(DEFAULT_MESSAGE_BACKLOG_SIZE) as u32,
            runtime: opts.runtime.unwrap_or_else(get_runtime_handle),
            listener: opts.listener,
            session_id: opts.session_id.unwrap_or_default(),
            name: opts.name.unwrap_or_default(),
            clients: CowVec::new(),
            channels: parking_lot::RwLock::new(HashMap::new()),
            capabilities: opts.capabilities.unwrap_or_default(),
            supported_encodings: opts.supported_encodings.unwrap_or_default(),
            cancellation_token: CancellationToken::new(),
        }
    }

    pub fn arc(&self) -> Arc<Self> {
        self.weak_self
            .upgrade()
            .expect("server cannot be dropped while in use")
    }

    // Returns a handle to the async runtime that this server is using.
    pub fn runtime(&self) -> &Handle {
        &self.runtime
    }

    // Spawn a task to accept all incoming connections and return
    pub async fn start(&self, host: &str, port: u16) -> Result<String, FoxgloveError> {
        if self.started.load(Acquire) {
            return Err(FoxgloveError::ServerAlreadyStarted);
        }
        let already_started = self.started.swap(true, AcqRel);
        assert!(!already_started);

        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(FoxgloveError::Bind)?;
        let bound_addr = listener
            .local_addr()
            .map_err(|err| FoxgloveError::Unspecified(err.into()))?
            .to_string();

        let cancellation_token = self.cancellation_token.clone();
        let server = self.arc().clone();
        self.runtime.spawn(async move {
            tokio::select! {
                () = handle_connections(server, listener) => (),
                () = cancellation_token.cancelled() => {
                    tracing::debug!("Closed connection handler");
                }
            }
        });

        Ok(bound_addr)
    }

    pub async fn stop(&self) {
        if self
            .started
            .compare_exchange(true, false, AcqRel, Acquire)
            .is_err()
        {
            return;
        }
        tracing::info!("Shutting down");
        let clients = self.clients.get();
        for client in clients.iter() {
            let mut sender = client.sender.lock().await;
            sender.send(Message::Close(None)).await.ok();
        }
        self.clients.clear();
        self.cancellation_token.cancel();
    }

    async fn advertise_channel(&self, channel: Arc<Channel>) {
        if channel.schema.is_none() {
            tracing::error!(
                "Ignoring advertise channel for {} because a schema is required",
                channel.topic
            );
            return;
        }

        self.channels.write().insert(channel.id, channel.clone());

        let message = match protocol::server::advertisement(&channel) {
            Ok(message) => message,
            Err(err) => {
                tracing::error!("Error creating advertise channel message to client: {err}");
                return;
            }
        };

        let clients = self.clients.get();
        for client in clients.iter() {
            if let Err(err) = client
                .control_plane_tx
                .send_async(Message::text(message.clone()))
                .await
            {
                tracing::error!("Error advertising channel to client {}: {err}", client.addr);
            } else {
                tracing::info!(
                    "Advertised channel {} with id {} to client {}",
                    channel.topic,
                    channel.id,
                    client.addr
                );
            }
        }
    }

    async fn unadvertise_channel(&self, channel_id: ChannelId) {
        self.channels.write().remove(&channel_id);

        let message = protocol::server::unadvertise(channel_id);
        let clients = self.clients.get();
        for client in clients.iter() {
            if let Err(err) = client
                .control_plane_tx
                .send_async(Message::text(message.clone()))
                .await
            {
                tracing::error!(
                    "Error unadvertising channel to client {}: {err}",
                    client.addr
                );
            } else {
                tracing::info!(
                    "Unadvertised channel with id {} to client {}",
                    channel_id,
                    client.addr
                );
            }
        }
    }

    /// Publish the current timestamp to all clients.
    #[cfg(feature = "unstable")]
    pub async fn broadcast_time(&self, timestamp_nanos: u64) {
        if !self.capabilities.contains(&Capability::Time) {
            tracing::error!("Server does not support time capability");
            return;
        }

        // https://github.com/foxglove/ws-protocol/blob/main/docs/spec.md#time
        let mut buf = BytesMut::with_capacity(9);
        buf.put_u8(protocol::server::BinaryOpcode::TimeData as u8);
        buf.put_u64_le(timestamp_nanos);
        let message = Message::binary(buf);

        let clients = self.clients.get();
        for client in clients.iter() {
            if let Err(err) = client.control_plane_tx.send_async(message.clone()).await {
                tracing::error!("Failed to send time to client {}: {err}", client.addr);
            }
        }
    }

    /// Publish parameter values to all clients.
    #[cfg(feature = "unstable")]
    pub async fn publish_parameter_values(
        &self,
        parameters: Vec<Parameter>,
        client_addr: Option<SocketAddr>,
    ) {
        if !self.capabilities.contains(&Capability::Parameters) {
            tracing::error!("Server does not support parameters capability");
            return;
        }

        let message = match protocol::server::parameters_json(parameters, None) {
            Ok(message) => message,
            Err(err) => {
                tracing::error!("Failed to serialize parameter values: {err}");
                return;
            }
        };
        // FG-9994: This should only send to clients that have subscribed to the parameters.
        let clients = self.clients.get();
        for client in clients.iter() {
            if client_addr.is_some_and(|addr| addr != client.addr) {
                continue;
            }
            if let Err(err) = client
                .control_plane_tx
                .send_async(Message::text(message.clone()))
                .await
            {
                tracing::error!(
                    "Failed to send parameter values to client {}: {err}",
                    client.addr
                );
            }
        }
    }

    /// When a new client connects:
    /// - Handshake
    /// - Send ServerInfo
    /// - Advertise existing channels
    /// - Listen for client meesages
    async fn handle_connection(self: Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        let ws_stream = match do_handshake(stream).await {
            Ok(ws_stream) => ws_stream,
            Err(_) => {
                tracing::error!("Dropping client {addr}: {}", WSError::HandshakeError);
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        let info_message = protocol::server::server_info(
            &self.session_id,
            &self.name,
            &self.capabilities,
            &self.supported_encodings,
        );
        if let Err(err) = ws_sender.send(Message::text(info_message)).await {
            // ServerInfo is required; do not store this client.
            tracing::error!("Failed to send required server info: {err}");
            return;
        }

        static CLIENT_ID: AtomicU32 = AtomicU32::new(1);
        let id = ClientId(CLIENT_ID.fetch_add(1, Relaxed));

        let (data_tx, data_rx) = flume::bounded(self.message_backlog_size as usize);
        let (ctrl_tx, ctrl_rx) = flume::bounded(DEFAULT_CONTROL_PLANE_BACKLOG_SIZE);

        let new_client = Arc::new(ConnectedClient {
            id,
            addr,
            sender: Mutex::new(ws_sender),
            data_plane_tx: data_tx,
            data_plane_rx: data_rx,
            control_plane_tx: ctrl_tx,
            control_plane_rx: ctrl_rx,
            subscriptions: parking_lot::Mutex::new(BiHashMap::new()),
            advertised_channels: parking_lot::Mutex::new(HashMap::new()),
            server_listener: self.listener.clone(),
            server: self.weak_self.clone(),
        });

        self.register_client_and_advertise_channels(new_client.clone())
            .await;

        let receive_messages = async {
            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(Message::Close(_)) => {
                        tracing::info!("Connection closed by client {addr}");
                        // Finish receive_messages
                        return;
                    }
                    Ok(msg) => {
                        new_client.handle_message(msg);
                    }
                    Err(err) => {
                        tracing::error!("Error receiving from client {addr}: {err}");
                    }
                }
            }
        };

        let send_control_messages = async {
            while let Ok(msg) = new_client.control_plane_rx.recv_async().await {
                let mut sender = new_client.sender.lock().await;
                if let Err(err) = sender.send(msg).await {
                    if self.started.load(Acquire) {
                        tracing::error!("Error sending control message to client {addr}: {err}");
                    } else {
                        new_client.control_plane_rx.drain();
                        new_client.data_plane_rx.drain();
                    }
                }
            }
        };

        // send_messages forwards messages from the rx size of the data plane to the sender
        let send_messages = async {
            while let Ok(msg) = new_client.data_plane_rx.recv_async().await {
                let mut sender = new_client.sender.lock().await;
                if let Err(err) = sender.send(msg).await {
                    if self.started.load(Acquire) {
                        tracing::error!("Error sending data message to client {addr}: {err}");
                    } else {
                        new_client.data_plane_rx.drain();
                        new_client.control_plane_rx.drain();
                    }
                }
            }
        };

        // Run send and receive loops concurrently, and wait for receive to complete
        tokio::select! {
            _ = receive_messages => {
                tracing::info!("Receive messages task completed");
            }
            _ = send_control_messages => {
                tracing::error!("Send control messages task completed");
            }
            _ = send_messages => {
                tracing::error!("Send messages task completed");
            }
        }

        self.clients.retain(|c| !Arc::ptr_eq(c, &new_client));
    }

    async fn register_client_and_advertise_channels(&self, client: Arc<ConnectedClient>) {
        // Lock the sender so the channel advertisement is the first message sent
        let mut sender = client.sender.lock().await;
        // Add the client to self.clients
        self.clients.push(client.clone());

        // Advertise existing channels to the new client. We must do this AFTER adding the client to clients,
        // otherwise there is potential for the client to miss a new channel advertisement.
        // Create a copy of the channels to avoid holding the lock while sending messages.
        let mut channels = Vec::<Arc<Channel>>::new();
        {
            let channels_map = self.channels.read();
            channels.extend(channels_map.values().cloned());
        }

        tracing::info!(
            "Registered client {} advertising {} channels",
            client.addr,
            channels.len()
        );

        for channel in channels.into_iter() {
            let message = match protocol::server::advertisement(&channel) {
                Ok(message) => message,
                Err(err) => {
                    tracing::error!("Error creating advertise channel message to client: {err}");
                    return;
                }
            };

            if let Err(err) = sender.send(Message::text(message)).await {
                // We can't send messages to the client. Maybe we can still receive messages? Let's continue.
                tracing::error!("Error advertising channel: {err}");
                break;
            }

            tracing::info!(
                "Advertised channel {} with id {} to client {}",
                channel.topic,
                channel.id,
                client.addr
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SendLossyResult {
    Sent,
    SentLossy(usize),
    ExhaustedRetries,
}

/// Attempt to send a message on the channel.
///
/// If the channel is non-full, this function returns `SendLossyResult::Sent`.
///
/// If the channel is full, drop the oldest message and try again. If the send eventually succeeds
/// in this manner, this function returns `SendLossyResult::SentLossy(dropped)`. If the maximum
/// number of retries is reached, it returns `SendLossyResult::ExhaustedRetries`.
fn send_lossy(
    tx: &flume::Sender<Message>,
    rx: &flume::Receiver<Message>,
    mut message: Message,
    retries: usize,
) -> SendLossyResult {
    let mut dropped = 0;
    loop {
        match (dropped, tx.try_send(message)) {
            (0, Ok(_)) => return SendLossyResult::Sent,
            (dropped, Ok(_)) => return SendLossyResult::SentLossy(dropped),
            (_, Err(TrySendError::Disconnected(_))) => unreachable!("we're holding rx"),
            (_, Err(TrySendError::Full(rejected))) => {
                if dropped >= retries {
                    return SendLossyResult::ExhaustedRetries;
                }
                message = rejected;
                let _ = rx.try_recv();
                dropped += 1
            }
        }
    }
}

impl LogSink for Server {
    fn log(
        &self,
        channel: &Arc<Channel>,
        msg: &[u8],
        metadata: &Metadata,
    ) -> Result<(), FoxgloveError> {
        let clients = self.clients.get();
        for client in clients.iter() {
            let subscriptions = client.subscriptions.lock();
            let Some(subscription_id) = subscriptions.get_by_left(&channel.id).cloned() else {
                continue;
            };

            // https://github.com/foxglove/ws-protocol/blob/main/docs/spec.md#message-data
            let header_size: usize = 1 + 4 + 8;
            let mut buf = BytesMut::with_capacity(header_size + msg.len());
            buf.put_u8(protocol::server::BinaryOpcode::MessageData as u8);
            buf.put_u32_le(subscription_id.into());
            buf.put_u64_le(metadata.log_time);
            buf.put_slice(msg);

            let message = Message::binary(buf);

            // If the queue is full, drop the oldest message(s). We do this because the websocket
            // client is falling behind, and we either start dropping messages, or we'll end up
            // buffering until we run out of memory. There's no point in that because the client is
            // unlikely to catch up and be able to consume the messages.
            match send_lossy(&client.data_plane_tx, &client.data_plane_rx, message, 10) {
                SendLossyResult::Sent => (),
                SendLossyResult::SentLossy(dropped) => tracing::warn!(
                    "outbox for client {} full, dropped {dropped} messages",
                    client.addr
                ),
                SendLossyResult::ExhaustedRetries => {
                    tracing::warn!(
                        "outbox for client {} full, dropping message after 10 attempts",
                        client.addr
                    )
                }
            };
        }
        Ok(())
    }

    /// Server has an available channel. Advertise to all clients.
    fn add_channel(&self, channel: &Arc<Channel>) {
        let server = self.arc();
        let ch = channel.clone();
        self.runtime
            .spawn(async move { server.advertise_channel(ch).await });
    }

    /// A channel is being removed. Unadvertise to all clients.
    fn remove_channel(&self, channel: &Channel) {
        let server = self.arc();
        let channel_id = channel.id();
        self.runtime
            .spawn(async move { server.unadvertise_channel(channel_id).await });
    }
}

pub(crate) fn create_server(opts: ServerOptions) -> Arc<Server> {
    Arc::new_cyclic(|weak_self| Server::new(weak_self.clone(), opts))
}

// Spawn a new task for each incoming connection
async fn handle_connections(server: Arc<Server>, listener: TcpListener) {
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(server.clone().handle_connection(stream, addr));
    }
}

/// Add the subprotocol header to the response if the client requested one we support.
/// If the client doesn't support our protocol, do not include the protocol header in the response;
/// the client must fail the connection. [WebSocket RFC](https://www.rfc-editor.org/rfc/rfc6455#section-4)
async fn do_handshake(stream: TcpStream) -> Result<WebSocketStream<TcpStream>, tungstenite::Error> {
    tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &server::Request, mut res: server::Response| {
            let all_headers = req.headers().get_all("sec-websocket-protocol");
            if all_headers.iter().any(|h| {
                (*h).to_str()
                    .unwrap_or_default()
                    .split(',')
                    .any(|s| s.trim() == SUBPROTOCOL)
            }) {
                res.headers_mut().insert(
                    "sec-websocket-protocol",
                    HeaderValue::from_static(SUBPROTOCOL),
                );
            };
            Ok(res)
        },
    )
    .await
}
