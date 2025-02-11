//! Websocket server

use std::fmt::Debug;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::runtime::Handle;

use crate::websocket::{create_server, Server, ServerOptions};
#[cfg(feature = "unstable")]
use crate::websocket::{Capability, Parameter};
use crate::{get_runtime_handle, FoxgloveError, LogContext, LogSink};

/// A websocket server for live visualization.
#[must_use]
#[derive(Debug)]
pub struct WebSocketServer {
    host: String,
    port: u16,
    options: ServerOptions,
}

impl Default for WebSocketServer {
    fn default() -> Self {
        let session_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis().to_string());
        let options = ServerOptions {
            session_id,
            ..ServerOptions::default()
        };
        Self {
            host: "127.0.0.1".into(),
            port: 8765,
            options,
        }
    }
}

impl WebSocketServer {
    /// Creates a new websocket server with default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the websocket server name to advertise to clients.
    ///
    /// By default, the server is not given a name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.options.name = Some(name.into());
        self
    }

    /// Bind a TCP port.
    ///
    /// By default, the server will bind to `127.0.0.1:8765`.
    pub fn bind(mut self, host: impl Into<String>, port: u16) -> Self {
        self.host = host.into();
        self.port = port;
        self
    }

    /// Sets the server capabilities to advertise to the client.
    ///
    /// By default, the server does not advertise any capabilities.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn capabilities(mut self, capabilities: impl IntoIterator<Item = Capability>) -> Self {
        self.options.capabilities = Some(capabilities.into_iter().collect());
        self
    }

    /// Configure an event listener to receive client message events.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn listener(mut self, listener: Arc<dyn crate::websocket::ServerListener>) -> Self {
        self.options.listener = Some(listener);
        self
    }

    /// Set the message backlog size.
    ///
    /// The server buffers outgoing log entries into a queue. If the backlog size is exceeded, the
    /// oldest entries will be dropped.
    ///
    /// By default, the server will buffer 1024 messages.
    pub fn message_backlog_size(mut self, size: usize) -> Self {
        self.options.message_backlog_size = Some(size);
        self
    }

    /// Set a session ID.
    ///
    /// This allows the client to understand if the connection is a re-connection or if it is
    /// connecting to a new server instance. This can for example be a timestamp or a UUID.
    ///
    /// By default, this is set to the number of milliseconds since the unix epoch.
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.options.session_id = Some(id.into());
        self
    }

    /// Configure the tokio runtime for the server to use for async tasks.
    ///
    /// By default, the server will use either the current runtime (if started with
    /// [`WebSocketServer::start`]), or spawn its own internal runtime (if started with
    /// [`WebSocketServer::start_blocking`]).
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn tokio_runtime(mut self, handle: &Handle) -> Self {
        self.options.runtime = Some(handle.clone());
        self
    }

    /// Starts the websocket server.
    ///
    /// Returns a handle that can optionally be used to gracefully shutdown the server. The caller
    /// can safely drop the handle, and the server will run forever.
    pub async fn start(self) -> Result<WebSocketServerHandle, FoxgloveError> {
        let server = create_server(self.options);
        server.start(&self.host, self.port).await?;
        LogContext::global().add_sink(server.clone());
        Ok(WebSocketServerHandle(server))
    }

    /// Starts the websocket server.
    ///
    /// Returns a handle that can optionally be used to gracefully shutdown the server. The caller
    /// can safely drop the handle, and the server will run forever.
    ///
    /// This method will panic if invoked from an asynchronous execution context. Use
    /// [`WebSocketServer::start`] instead.
    pub fn start_blocking(mut self) -> Result<WebSocketServerBlockingHandle, FoxgloveError> {
        let runtime = self
            .options
            .runtime
            .get_or_insert_with(get_runtime_handle)
            .clone();
        let handle = runtime.block_on(self.start())?;
        Ok(WebSocketServerBlockingHandle(handle))
    }
}

/// A handle to the websocket server.
///
/// This handle can safely be dropped and the server will run forever.
pub struct WebSocketServerHandle(Arc<Server>);

impl Debug for WebSocketServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WebSocketServerHandle").finish()
    }
}

impl WebSocketServerHandle {
    /// Returns a handle to the async runtime.
    fn runtime(&self) -> &Handle {
        self.0.runtime()
    }

    /// Publishes the current server timestamp to all clients.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub async fn broadcast_time(&self, timestamp_nanos: u64) {
        self.0.broadcast_time(timestamp_nanos).await;
    }

    /// Publishes parameter values to all clients.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub async fn publish_parameter_values(&self, parameters: impl IntoIterator<Item = Parameter>) {
        self.0
            .publish_parameter_values(parameters.into_iter().collect(), None)
            .await;
    }

    /// Gracefully shutdown the websocket server.
    pub async fn stop(self) {
        let sink = self.0.clone() as Arc<dyn LogSink>;
        LogContext::global().remove_sink(&sink);
        self.0.stop().await;
    }
}

/// A blocking wrapper around a WebSocketServerHandle.
#[derive(Debug)]
pub struct WebSocketServerBlockingHandle(WebSocketServerHandle);

impl WebSocketServerBlockingHandle {
    /// Publishes the current server timestamp to all clients.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn broadcast_time(&self, timestamp_nanos: u64) {
        self.0
            .runtime()
            .block_on(self.0.broadcast_time(timestamp_nanos))
    }

    /// Publishes parameter values to all clients.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn publish_parameter_values(&self, parameters: impl IntoIterator<Item = Parameter>) {
        self.0
            .runtime()
            .block_on(self.0.publish_parameter_values(parameters))
    }

    /// Gracefully shutdown the websocket server.
    pub fn stop(self) {
        self.0.runtime().clone().block_on(self.0.stop());
    }
}
