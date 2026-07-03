//! State management for the Network ability.
//!
//! This module provides thread-safe state for managing TCP listeners and connections
//! used by the Network ability handlers.
//!
//! This is a pure networking layer that handles:
//! - TCP listener lifecycle
//! - TCP connection lifecycle
//! - Length-prefixed message I/O

use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle as RuntimeHandle;

/// Handle type for listeners.
pub type ListenerId = u64;

/// Handle type for connections.
pub type ConnectionId = u64;

/// Maximum message size (16 MB).
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// A TCP connection managed by the Network ability.
pub struct NetworkConnection {
    /// The underlying TCP stream.
    pub stream: TcpStream,
    /// Cached local address.
    pub local_addr: String,
    /// Cached peer address.
    pub peer_addr: String,
}

/// Thread-safe state for Network ability handlers.
///
/// This state is shared across all Network ability method handlers and tracks:
/// - Active TCP listeners
/// - Active connections
/// - Handle ID generation
pub struct NetworkState {
    /// Active listeners by ID.
    listeners: HashMap<ListenerId, TcpListener>,
    /// Active connections by ID.
    connections: HashMap<ConnectionId, NetworkConnection>,
    /// Next handle ID to allocate.
    next_id: u64,
    /// Tokio runtime handle for async operations.
    runtime: RuntimeHandle,
}

impl NetworkState {
    /// Create new network state with the given runtime.
    #[must_use]
    pub fn new(runtime: RuntimeHandle) -> Self {
        Self {
            listeners: HashMap::new(),
            connections: HashMap::new(),
            next_id: 1, // Start at 1 so 0 can be used as "invalid"
            runtime,
        }
    }

    /// Allocate and return the next handle ID.
    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Get the runtime handle.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Listener operations
    // ═══════════════════════════════════════════════════════════════════════════

    /// Bind a TCP listener to the given address.
    ///
    /// Returns the listener ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the address cannot be bound.
    pub fn listen(&mut self, addr: &str) -> Result<ListenerId, NetworkError> {
        let listener = self
            .runtime
            .block_on(TcpListener::bind(addr))
            .map_err(NetworkError::Io)?;
        let id = self.next_id();
        self.listeners.insert(id, listener);
        Ok(id)
    }

    /// Accept a connection on the given listener.
    ///
    /// Returns the connection ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid or accepting fails.
    pub fn accept(&mut self, listener_id: ListenerId) -> Result<ConnectionId, NetworkError> {
        let listener = self
            .listeners
            .get(&listener_id)
            .ok_or(NetworkError::InvalidListener(listener_id))?;

        let (stream, peer_addr) = self
            .runtime
            .block_on(listener.accept())
            .map_err(NetworkError::Io)?;

        let local_addr = stream
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();

        let id = self.next_id();
        self.connections.insert(
            id,
            NetworkConnection {
                stream,
                local_addr,
                peer_addr: peer_addr.to_string(),
            },
        );
        Ok(id)
    }

    /// Close and remove a listener.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid.
    pub fn close_listener(&mut self, id: ListenerId) -> Result<(), NetworkError> {
        self.listeners
            .remove(&id)
            .map(|_| ())
            .ok_or(NetworkError::InvalidListener(id))
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Connection operations
    // ═══════════════════════════════════════════════════════════════════════════

    /// Connect to a remote server.
    ///
    /// Returns the connection ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established.
    pub fn connect(&mut self, addr: &str) -> Result<ConnectionId, NetworkError> {
        let stream = self
            .runtime
            .block_on(TcpStream::connect(addr))
            .map_err(NetworkError::Io)?;

        let local_addr = stream
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let peer_addr = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();

        let id = self.next_id();
        self.connections.insert(
            id,
            NetworkConnection {
                stream,
                local_addr,
                peer_addr,
            },
        );
        Ok(id)
    }

    /// Close and remove a connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn close(&mut self, id: ConnectionId) -> Result<(), NetworkError> {
        self.connections
            .remove(&id)
            .map(|_| ())
            .ok_or(NetworkError::InvalidConnection(id))
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Message I/O (length-prefixed)
    // ═══════════════════════════════════════════════════════════════════════════

    /// Send a length-prefixed message.
    ///
    /// Wire format: [4 bytes: length (big-endian u32)] [length bytes: payload]
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid, the message is too large,
    /// or writing fails.
    pub fn send(&mut self, conn_id: ConnectionId, data: &[u8]) -> Result<(), NetworkError> {
        let conn = self
            .connections
            .get_mut(&conn_id)
            .ok_or(NetworkError::InvalidConnection(conn_id))?;

        if data.len() > MAX_MESSAGE_SIZE as usize {
            return Err(NetworkError::MessageTooLarge(data.len()));
        }

        // Safe: we've verified data.len() <= MAX_MESSAGE_SIZE (u32::MAX >> 8)
        #[allow(clippy::cast_possible_truncation)]
        let len = data.len() as u32;
        let stream = &mut conn.stream;

        self.runtime.block_on(async {
            stream.write_all(&len.to_be_bytes()).await?;
            stream.write_all(data).await?;
            stream.flush().await?;
            Ok::<_, std::io::Error>(())
        })?;

        Ok(())
    }

    /// Receive a length-prefixed message.
    ///
    /// Wire format: [4 bytes: length (big-endian u32)] [length bytes: payload]
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid, the message is too large,
    /// or reading fails.
    pub fn receive(&mut self, conn_id: ConnectionId) -> Result<Vec<u8>, NetworkError> {
        let conn = self
            .connections
            .get_mut(&conn_id)
            .ok_or(NetworkError::InvalidConnection(conn_id))?;

        let stream = &mut conn.stream;

        self.runtime.block_on(async {
            // Read length prefix
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(NetworkError::ConnectionClosed);
                }
                Err(e) => return Err(NetworkError::Io(e)),
            }

            let len = u32::from_be_bytes(len_buf);
            if len > MAX_MESSAGE_SIZE {
                return Err(NetworkError::MessageTooLarge(len as usize));
            }

            // Read payload
            let mut buf = vec![0u8; len as usize];
            stream.read_exact(&mut buf).await?;
            Ok(buf)
        })
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Connection info
    // ═══════════════════════════════════════════════════════════════════════════

    /// Get the local address of a connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn local_addr(&self, conn_id: ConnectionId) -> Result<&str, NetworkError> {
        self.connections
            .get(&conn_id)
            .map(|c| c.local_addr.as_str())
            .ok_or(NetworkError::InvalidConnection(conn_id))
    }

    /// Get the peer address of a connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn peer_addr(&self, conn_id: ConnectionId) -> Result<&str, NetworkError> {
        self.connections
            .get(&conn_id)
            .map(|c| c.peer_addr.as_str())
            .ok_or(NetworkError::InvalidConnection(conn_id))
    }
}

/// Errors that can occur in network operations.
#[derive(Debug)]
pub enum NetworkError {
    /// I/O error.
    Io(std::io::Error),
    /// Invalid listener ID.
    InvalidListener(ListenerId),
    /// Invalid connection ID.
    InvalidConnection(ConnectionId),
    /// Connection was closed.
    ConnectionClosed,
    /// Message exceeds maximum size.
    MessageTooLarge(usize),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidListener(id) => write!(f, "invalid listener ID: {id}"),
            Self::InvalidConnection(id) => write!(f, "invalid connection ID: {id}"),
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::MessageTooLarge(size) => {
                write!(
                    f,
                    "message too large: {size} bytes (max {MAX_MESSAGE_SIZE})"
                )
            }
        }
    }
}

impl std::error::Error for NetworkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for NetworkError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_id_increments() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut state = NetworkState::new(runtime.handle().clone());

        assert_eq!(state.next_id(), 1);
        assert_eq!(state.next_id(), 2);
        assert_eq!(state.next_id(), 3);
    }

    #[test]
    fn test_invalid_listener_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut state = NetworkState::new(runtime.handle().clone());

        let result = state.accept(999);
        assert!(matches!(result, Err(NetworkError::InvalidListener(999))));

        let result = state.close_listener(999);
        assert!(matches!(result, Err(NetworkError::InvalidListener(999))));
    }

    #[test]
    fn test_invalid_connection_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut state = NetworkState::new(runtime.handle().clone());

        let result = state.close(999);
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));

        let result = state.send(999, b"hello");
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));

        let result = state.receive(999);
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));
    }

    #[test]
    fn test_error_display() {
        assert_eq!(
            NetworkError::InvalidListener(42).to_string(),
            "invalid listener ID: 42"
        );
        assert_eq!(
            NetworkError::InvalidConnection(99).to_string(),
            "invalid connection ID: 99"
        );
        assert_eq!(
            NetworkError::ConnectionClosed.to_string(),
            "connection closed"
        );
        assert!(
            NetworkError::MessageTooLarge(1000)
                .to_string()
                .contains("1000")
        );
    }
}
