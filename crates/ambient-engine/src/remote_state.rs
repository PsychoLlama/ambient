//! State management for the Remote ability.
//!
//! This module provides thread-safe state for managing TCP listeners and connections
//! used by the Remote ability handlers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle as RuntimeHandle;

use crate::abilities::register_all_standard_abilities;
use crate::remote::Executor;
use crate::store::Store;

/// Handle type for listeners.
pub type ListenerId = u64;

/// Handle type for connections.
pub type ConnectionId = u64;

/// A TCP connection with optional server-side executor.
pub struct Connection {
    /// The TCP stream for this connection.
    pub stream: TcpStream,
    /// Server-side connections have an executor for running received functions.
    /// Client-side connections have None.
    pub executor: Option<Executor>,
}

/// Thread-safe state for Remote ability handlers.
///
/// This state is shared across all Remote ability method handlers and tracks:
/// - Active TCP listeners
/// - Active connections (both client and server-side)
/// - Handle ID generation
pub struct RemoteState {
    /// Active listeners by ID.
    listeners: HashMap<ListenerId, TcpListener>,
    /// Active connections by ID.
    connections: HashMap<ConnectionId, Connection>,
    /// Next handle ID to allocate.
    next_id: u64,
    /// Tokio runtime handle for async operations.
    runtime: RuntimeHandle,
    /// Store for function lookup (client needs this to send dependencies).
    store: Arc<Mutex<Store>>,
}

impl RemoteState {
    /// Create new remote state with the given runtime and store.
    pub fn new(runtime: RuntimeHandle, store: Arc<Mutex<Store>>) -> Self {
        Self {
            listeners: HashMap::new(),
            connections: HashMap::new(),
            next_id: 1, // Start at 1 so 0 can be used as "invalid"
            runtime,
            store,
        }
    }

    /// Allocate and return the next handle ID.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Get the runtime handle.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    /// Get the store.
    #[must_use]
    pub fn store(&self) -> &Arc<Mutex<Store>> {
        &self.store
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Listener operations
    // ─────────────────────────────────────────────────────────────────────────

    /// Bind a TCP listener to the given address.
    ///
    /// Returns the listener ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the address cannot be bound.
    pub fn listen(&mut self, addr: &str) -> Result<ListenerId, std::io::Error> {
        let listener = self.runtime.block_on(TcpListener::bind(addr))?;
        let id = self.next_id();
        self.listeners.insert(id, listener);
        Ok(id)
    }

    /// Accept a connection on the given listener.
    ///
    /// Returns the connection ID on success. The connection is server-side
    /// (has an executor for running remote functions).
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid or accepting fails.
    pub fn accept(&mut self, listener_id: ListenerId) -> Result<ConnectionId, RemoteStateError> {
        let listener = self
            .listeners
            .get(&listener_id)
            .ok_or(RemoteStateError::InvalidListener(listener_id))?;

        let (stream, _addr) = self
            .runtime
            .block_on(listener.accept())
            .map_err(RemoteStateError::Io)?;

        // Create an executor for server-side connections
        let mut executor = Executor::new();
        // Register standard abilities so remote code can use Console, etc.
        register_all_standard_abilities(executor.vm_mut());

        let id = self.next_id();
        self.connections.insert(
            id,
            Connection {
                stream,
                executor: Some(executor),
            },
        );
        Ok(id)
    }

    /// Get a listener by ID.
    #[must_use]
    pub fn get_listener(&self, id: ListenerId) -> Option<&TcpListener> {
        self.listeners.get(&id)
    }

    /// Remove and return a listener.
    pub fn remove_listener(&mut self, id: ListenerId) -> Option<TcpListener> {
        self.listeners.remove(&id)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Connection operations
    // ─────────────────────────────────────────────────────────────────────────

    /// Connect to a remote server.
    ///
    /// Returns the connection ID on success. The connection is client-side
    /// (no executor).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established.
    pub fn connect(&mut self, addr: &str) -> Result<ConnectionId, RemoteStateError> {
        let stream = self
            .runtime
            .block_on(TcpStream::connect(addr))
            .map_err(RemoteStateError::Io)?;

        let id = self.next_id();
        self.connections.insert(
            id,
            Connection {
                stream,
                executor: None, // Client-side has no executor
            },
        );
        Ok(id)
    }

    /// Get a connection by ID.
    #[must_use]
    pub fn get_connection(&self, id: ConnectionId) -> Option<&Connection> {
        self.connections.get(&id)
    }

    /// Get a mutable connection by ID.
    pub fn get_connection_mut(&mut self, id: ConnectionId) -> Option<&mut Connection> {
        self.connections.get_mut(&id)
    }

    /// Remove and return a connection.
    pub fn remove_connection(&mut self, id: ConnectionId) -> Option<Connection> {
        self.connections.remove(&id)
    }

    /// Close a connection by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn close(&mut self, id: ConnectionId) -> Result<(), RemoteStateError> {
        self.connections
            .remove(&id)
            .map(|_| ())
            .ok_or(RemoteStateError::InvalidConnection(id))
    }
}

/// Errors that can occur in remote state operations.
#[derive(Debug)]
pub enum RemoteStateError {
    /// I/O error.
    Io(std::io::Error),
    /// Invalid listener ID.
    InvalidListener(ListenerId),
    /// Invalid connection ID.
    InvalidConnection(ConnectionId),
    /// Tried to use a client connection as a server.
    NotServerConnection,
    /// Connection was closed.
    ConnectionClosed,
    /// Protocol error.
    Protocol(String),
}

impl std::fmt::Display for RemoteStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidListener(id) => write!(f, "invalid listener ID: {id}"),
            Self::InvalidConnection(id) => write!(f, "invalid connection ID: {id}"),
            Self::NotServerConnection => write!(f, "connection is not a server connection"),
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}

impl std::error::Error for RemoteStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RemoteStateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
