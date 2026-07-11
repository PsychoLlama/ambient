//! State management for the Network ability.
//!
//! This module provides thread-safe state for managing TCP listeners and connections
//! used by the Network ability handlers.
//!
//! This is a pure networking layer that handles:
//! - TCP listener lifecycle
//! - TCP connection lifecycle
//! - Length-prefixed message I/O
//!
//! One `NetworkState` is shared by every VM in a process runtime, so a
//! listener bound in one process can be accepted in another and a
//! connection handle can be handed between processes as a plain number.
//! The handle table lock is held only for lookups; blocking IO happens
//! under per-listener/per-connection-half locks, so a process blocked in
//! `accept` or `receive` never stalls network operations elsewhere.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle as RuntimeHandle;

/// Handle type for listeners.
pub type ListenerId = u64;

/// Handle type for connections.
pub type ConnectionId = u64;

/// Maximum message size (16 MB).
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// How many bytes one raw (unframed) read can return at most.
const RAW_READ_BUFFER: usize = 64 * 1024;

/// A TCP connection managed by the Network ability.
///
/// The stream is split so that a receiver blocked waiting for bytes does
/// not lock out a sender on the same connection (full-duplex use from
/// two processes).
struct NetworkConnection {
    /// Read half, locked by the (single) blocked receiver.
    read: Mutex<OwnedReadHalf>,
    /// Write half, locked per send.
    write: Mutex<OwnedWriteHalf>,
    /// Cached local address.
    local_addr: String,
    /// Cached peer address.
    peer_addr: String,
}

/// The handle tables. Locked only for lookup/insert/remove — never while
/// blocking on IO.
#[derive(Default)]
struct Tables {
    /// Active listeners by ID.
    listeners: HashMap<ListenerId, Arc<TcpListener>>,
    /// Active connections by ID.
    connections: HashMap<ConnectionId, Arc<NetworkConnection>>,
    /// Next handle ID to allocate.
    next_id: u64,
}

/// Thread-safe state for Network ability handlers.
///
/// This state is shared across all Network ability method handlers (and,
/// under the process runtime, across all process VMs) and tracks:
/// - Active TCP listeners
/// - Active connections
/// - Handle ID generation
pub struct NetworkState {
    /// Handle tables behind a short-lived lock.
    tables: Mutex<Tables>,
    /// Tokio runtime handle for async operations.
    runtime: RuntimeHandle,
}

impl NetworkState {
    /// Create new network state with the given runtime.
    #[must_use]
    pub fn new(runtime: RuntimeHandle) -> Self {
        Self {
            tables: Mutex::new(Tables {
                listeners: HashMap::new(),
                connections: HashMap::new(),
                next_id: 1, // Start at 1 so 0 can be used as "invalid"
            }),
            runtime,
        }
    }

    /// Get the runtime handle.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    fn lock_tables(&self) -> Result<std::sync::MutexGuard<'_, Tables>, NetworkError> {
        self.tables.lock().map_err(|_| NetworkError::Poisoned)
    }

    fn listener(&self, id: ListenerId) -> Result<Arc<TcpListener>, NetworkError> {
        self.lock_tables()?
            .listeners
            .get(&id)
            .cloned()
            .ok_or(NetworkError::InvalidListener(id))
    }

    fn connection(&self, id: ConnectionId) -> Result<Arc<NetworkConnection>, NetworkError> {
        self.lock_tables()?
            .connections
            .get(&id)
            .cloned()
            .ok_or(NetworkError::InvalidConnection(id))
    }

    /// Register a connected stream and return its handle.
    fn insert_stream(&self, stream: TcpStream) -> Result<ConnectionId, NetworkError> {
        let local_addr = stream
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let peer_addr = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let (read, write) = stream.into_split();

        let mut tables = self.lock_tables()?;
        let id = tables.next_id;
        tables.next_id += 1;
        tables.connections.insert(
            id,
            Arc::new(NetworkConnection {
                read: Mutex::new(read),
                write: Mutex::new(write),
                local_addr,
                peer_addr,
            }),
        );
        Ok(id)
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Listener operations
    // ═══════════════════════════════════════════════════════════════════════════

    /// Bind a TCP listener to the given `(host, port)` endpoint.
    ///
    /// Returns the listener ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint cannot be bound.
    pub fn listen(&self, host: &str, port: u16) -> Result<ListenerId, NetworkError> {
        let listener = self
            .runtime
            .block_on(TcpListener::bind((host, port)))
            .map_err(NetworkError::Io)?;
        let mut tables = self.lock_tables()?;
        let id = tables.next_id;
        tables.next_id += 1;
        tables.listeners.insert(id, Arc::new(listener));
        Ok(id)
    }

    /// Accept a connection on the given listener, blocking until a client
    /// connects.
    ///
    /// Returns the connection ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid or accepting fails.
    pub fn accept(&self, listener_id: ListenerId) -> Result<ConnectionId, NetworkError> {
        self.accept_interruptible(listener_id, std::future::pending())
    }

    /// [`Self::accept`], except the wait also races `cancel`: when it
    /// completes first, the accept is abandoned and the call returns
    /// [`NetworkError::Interrupted`]. This is the interruption point for
    /// a drain request — the caller (an interruptible native) turns the
    /// error into a `Drain::requested` delivery.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid, accepting fails,
    /// or `cancel` completes first ([`NetworkError::Interrupted`]).
    pub fn accept_interruptible(
        &self,
        listener_id: ListenerId,
        cancel: impl Future<Output = ()>,
    ) -> Result<ConnectionId, NetworkError> {
        let listener = self.listener(listener_id)?;
        let (stream, _peer) = self.runtime.block_on(async {
            tokio::select! {
                result = listener.accept() => result.map_err(NetworkError::Io),
                () = cancel => Err(NetworkError::Interrupted),
            }
        })?;
        self.insert_stream(stream)
    }

    /// The local address a listener is bound to (e.g. to learn the OS-
    /// assigned port after binding port 0).
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid or the socket has no
    /// local address.
    pub fn listener_addr(&self, id: ListenerId) -> Result<String, NetworkError> {
        let addr = self.listener(id)?.local_addr().map_err(NetworkError::Io)?;
        Ok(addr.to_string())
    }

    /// Close and remove a listener.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener ID is invalid.
    pub fn close_listener(&self, id: ListenerId) -> Result<(), NetworkError> {
        self.lock_tables()?
            .listeners
            .remove(&id)
            .map(|_| ())
            .ok_or(NetworkError::InvalidListener(id))
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Connection operations
    // ═══════════════════════════════════════════════════════════════════════════

    /// Connect to a remote server at the given `(host, port)` endpoint.
    ///
    /// Returns the connection ID on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established.
    pub fn connect(&self, host: &str, port: u16) -> Result<ConnectionId, NetworkError> {
        let stream = self
            .runtime
            .block_on(TcpStream::connect((host, port)))
            .map_err(NetworkError::Io)?;
        self.insert_stream(stream)
    }

    /// Close and remove a connection.
    ///
    /// The socket closes when the last in-flight operation on it
    /// finishes (a receiver blocked on the connection keeps it alive
    /// until its read returns).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn close(&self, id: ConnectionId) -> Result<(), NetworkError> {
        self.lock_tables()?
            .connections
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
    pub fn send(&self, conn_id: ConnectionId, data: &[u8]) -> Result<(), NetworkError> {
        if data.len() > MAX_MESSAGE_SIZE as usize {
            return Err(NetworkError::MessageTooLarge(data.len()));
        }

        let conn = self.connection(conn_id)?;
        let mut write = conn.write.lock().map_err(|_| NetworkError::Poisoned)?;

        // Safe: we've verified data.len() <= MAX_MESSAGE_SIZE (u32::MAX >> 8)
        #[allow(clippy::cast_possible_truncation)]
        let len = data.len() as u32;

        self.runtime.block_on(async {
            write.write_all(&len.to_be_bytes()).await?;
            write.write_all(data).await?;
            write.flush().await?;
            Ok::<_, std::io::Error>(())
        })?;

        Ok(())
    }

    /// Receive a length-prefixed message, blocking until one arrives.
    ///
    /// Wire format: [4 bytes: length (big-endian u32)] [length bytes: payload]
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid, the message is too large,
    /// or reading fails.
    pub fn receive(&self, conn_id: ConnectionId) -> Result<Vec<u8>, NetworkError> {
        self.receive_interruptible(conn_id, std::future::pending())
    }

    /// [`Self::receive`], except the wait also races `cancel`: when it
    /// completes first, the read is abandoned and the call returns
    /// [`NetworkError::Interrupted`]. An interrupted receive may abandon
    /// a partially-read frame, corrupting the stream's framing — a drain
    /// means the connection is being torn down, so the caller must not
    /// reuse it.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid, the message is too
    /// large, reading fails, or `cancel` completes first
    /// ([`NetworkError::Interrupted`]).
    pub fn receive_interruptible(
        &self,
        conn_id: ConnectionId,
        cancel: impl Future<Output = ()>,
    ) -> Result<Vec<u8>, NetworkError> {
        let conn = self.connection(conn_id)?;
        let mut read = conn.read.lock().map_err(|_| NetworkError::Poisoned)?;

        self.runtime.block_on(async {
            let receive = async {
                // Read length prefix
                let mut len_buf = [0u8; 4];
                match read.read_exact(&mut len_buf).await {
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
                read.read_exact(&mut buf).await?;
                Ok(buf)
            };
            tokio::select! {
                result = receive => result,
                () = cancel => Err(NetworkError::Interrupted),
            }
        })
    }

    /// Write raw bytes to a connection — no length prefix, no framing.
    /// The unframed counterpart of [`Self::send`], for speaking foreign
    /// wire protocols (HTTP, redis, ...). Never mix framed and raw calls
    /// on one connection: the framed side would read payload bytes as a
    /// length.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid or writing fails.
    pub fn send_raw(&self, conn_id: ConnectionId, data: &[u8]) -> Result<(), NetworkError> {
        let conn = self.connection(conn_id)?;
        let mut write = conn.write.lock().map_err(|_| NetworkError::Poisoned)?;
        self.runtime.block_on(async {
            write.write_all(data).await?;
            write.flush().await?;
            Ok::<_, std::io::Error>(())
        })?;
        Ok(())
    }

    /// Read whatever bytes are next on a connection — no length prefix,
    /// no framing — blocking until at least one byte arrives. Returns an
    /// empty buffer when the peer closed the connection (raw reads have
    /// no frame boundary, so EOF is data, not an error).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid or reading fails.
    pub fn receive_raw(&self, conn_id: ConnectionId) -> Result<Vec<u8>, NetworkError> {
        self.receive_raw_interruptible(conn_id, std::future::pending())
    }

    /// [`Self::receive_raw`], except the wait also races `cancel`: when
    /// it completes first, the read is abandoned and the call returns
    /// [`NetworkError::Interrupted`] — the same drain contract as
    /// [`Self::receive_interruptible`].
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is invalid, reading fails, or
    /// `cancel` completes first ([`NetworkError::Interrupted`]).
    pub fn receive_raw_interruptible(
        &self,
        conn_id: ConnectionId,
        cancel: impl Future<Output = ()>,
    ) -> Result<Vec<u8>, NetworkError> {
        let conn = self.connection(conn_id)?;
        let mut read = conn.read.lock().map_err(|_| NetworkError::Poisoned)?;

        self.runtime.block_on(async {
            let receive = async {
                let mut buf = vec![0u8; RAW_READ_BUFFER];
                let n = read.read(&mut buf).await?;
                buf.truncate(n);
                Ok(buf)
            };
            tokio::select! {
                result = receive => result,
                () = cancel => Err(NetworkError::Interrupted),
            }
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
    pub fn local_addr(&self, conn_id: ConnectionId) -> Result<String, NetworkError> {
        Ok(self.connection(conn_id)?.local_addr.clone())
    }

    /// Get the peer address of a connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection ID is invalid.
    pub fn peer_addr(&self, conn_id: ConnectionId) -> Result<String, NetworkError> {
        Ok(self.connection(conn_id)?.peer_addr.clone())
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
    /// A blocking operation was abandoned because its cancel future won
    /// the race (a drain request).
    Interrupted,
    /// A lock was poisoned by a panicking thread.
    Poisoned,
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
            Self::Interrupted => write!(f, "operation interrupted by a drain request"),
            Self::Poisoned => write!(f, "network state lock poisoned"),
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
    fn test_invalid_listener_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = NetworkState::new(runtime.handle().clone());

        let result = state.accept(999);
        assert!(matches!(result, Err(NetworkError::InvalidListener(999))));

        let result = state.close_listener(999);
        assert!(matches!(result, Err(NetworkError::InvalidListener(999))));
    }

    #[test]
    fn test_invalid_connection_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = NetworkState::new(runtime.handle().clone());

        let result = state.close(999);
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));

        let result = state.send(999, b"hello");
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));

        let result = state.receive(999);
        assert!(matches!(result, Err(NetworkError::InvalidConnection(999))));
    }

    #[test]
    fn test_handles_shared_across_threads() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = Arc::new(NetworkState::new(runtime.handle().clone()));

        let listener = state.listen("127.0.0.1", 0).unwrap();

        // Accept on one thread while connecting from another: the
        // table lock must not serialize blocking operations.
        let port = {
            let tables = state.lock_tables().unwrap();
            tables.listeners[&listener].local_addr().unwrap().port()
        };

        let accept_state = Arc::clone(&state);
        let acceptor = std::thread::spawn(move || accept_state.accept(listener).unwrap());

        let client = state.connect("127.0.0.1", port).unwrap();
        let server = acceptor.join().unwrap();

        state.send(client, b"ping").unwrap();
        let got = state.receive(server).unwrap();
        assert_eq!(got, b"ping");

        // Full duplex: reply while another thread blocks reading.
        let reply_state = Arc::clone(&state);
        let reader = std::thread::spawn(move || reply_state.receive(client).unwrap());
        state.send(server, b"pong").unwrap();
        assert_eq!(reader.join().unwrap(), b"pong");

        state.close(client).unwrap();
        state.close(server).unwrap();
        state.close_listener(listener).unwrap();
    }

    #[test]
    fn raw_bytes_round_trip_without_framing() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = Arc::new(NetworkState::new(runtime.handle().clone()));

        let listener = state.listen("127.0.0.1", 0).unwrap();
        let port = {
            let tables = state.lock_tables().unwrap();
            tables.listeners[&listener].local_addr().unwrap().port()
        };
        let accept_state = Arc::clone(&state);
        let acceptor = std::thread::spawn(move || accept_state.accept(listener).unwrap());
        let client = state.connect("127.0.0.1", port).unwrap();
        let server = acceptor.join().unwrap();

        // Raw bytes cross exactly as written: no 4-byte length prefix.
        state.send_raw(client, b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let got = state.receive_raw(server).unwrap();
        assert_eq!(got, b"GET / HTTP/1.0\r\n\r\n");

        // A closed peer reads as an empty buffer (EOF is data, not an
        // error — raw reads have no frame boundary to violate).
        state.close(client).unwrap();
        let got = state.receive_raw(server).unwrap();
        assert!(got.is_empty());

        state.close(server).unwrap();
        state.close_listener(listener).unwrap();
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
