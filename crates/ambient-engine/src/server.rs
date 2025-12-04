//! TCP server for remote function execution.
//!
//! The server listens for incoming connections and executes functions on behalf
//! of remote clients. It maintains a local store of functions and can request
//! missing dependencies from clients.
//!
//! # Example
//!
//! ```ignore
//! use ambient_engine::server::Server;
//! use ambient_engine::abilities::console;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut server = Server::new();
//!
//!     // Register ability handlers
//!     console::register_console(server.vm_mut(), Default::default());
//!
//!     // Start listening
//!     server.listen("127.0.0.1:8080").await.unwrap();
//! }
//! ```

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::Mutex;

use crate::bytecode::CompiledFunction;
use crate::protocol::{read_message, write_message, ErrorKind, Message, ProtocolError};
use crate::remote::Executor;
use crate::store::{PortableFunction, Store, StoreError};
use crate::value::Value;

/// Errors that can occur during server operations.
#[derive(Debug)]
pub enum ServerError {
    /// I/O error.
    Io(std::io::Error),
    /// Protocol error.
    Protocol(ProtocolError),
    /// Store error.
    Store(StoreError),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Protocol(e) => write!(f, "protocol error: {e}"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Protocol(e) => Some(e),
            Self::Store(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for ServerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ProtocolError> for ServerError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

impl From<StoreError> for ServerError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// A TCP server for remote function execution.
///
/// The server maintains an executor with a function store and VM instance.
/// Multiple clients can connect and execute functions concurrently (each
/// connection is handled sequentially, but multiple connections run in parallel).
pub struct Server {
    /// The executor handles function storage and execution.
    executor: Arc<Mutex<Executor>>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Create a new server with an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            executor: Arc::new(Mutex::new(Executor::new())),
        }
    }

    /// Create a server with a pre-populated store.
    #[must_use]
    pub fn with_store(store: Store) -> Self {
        Self {
            executor: Arc::new(Mutex::new(Executor::with_store(store))),
        }
    }

    /// Get mutable access to the executor for configuration.
    ///
    /// Use this to register ability handlers before starting the server.
    pub async fn executor_mut(&self) -> tokio::sync::MutexGuard<'_, Executor> {
        self.executor.lock().await
    }

    /// Listen for connections and handle them.
    ///
    /// This method runs forever, accepting connections and spawning tasks to handle them.
    pub async fn listen(&self, addr: impl ToSocketAddrs) -> Result<(), ServerError> {
        let listener = TcpListener::bind(addr).await?;

        loop {
            let (stream, peer_addr) = listener.accept().await?;

            // Clone the executor handle for the connection handler
            let executor = Arc::clone(&self.executor);

            // Spawn a task to handle this connection
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, executor).await {
                    #[allow(clippy::print_stderr)]
                    {
                        eprintln!("Error handling connection from {peer_addr}: {e}");
                    }
                }
            });
        }
    }

    /// Listen and handle a single connection (useful for testing).
    pub async fn handle_one(&self, addr: impl ToSocketAddrs) -> Result<(), ServerError> {
        let listener = TcpListener::bind(addr).await?;
        let (stream, _) = listener.accept().await?;
        handle_connection(stream, Arc::clone(&self.executor)).await
    }
}

/// Handle a single client connection.
async fn handle_connection(
    stream: TcpStream,
    executor: Arc<Mutex<Executor>>,
) -> Result<(), ServerError> {
    let (mut reader, mut writer) = stream.into_split();
    handle_session(&mut reader, &mut writer, executor).await
}

/// Handle a session with a client.
///
/// This is separate from `handle_connection` to allow testing with mock streams.
pub async fn handle_session<R, W>(
    reader: &mut R,
    writer: &mut W,
    executor: Arc<Mutex<Executor>>,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        // Read the next message
        let Some(message) = read_message(reader).await? else {
            return Ok(()); // Clean disconnect
        };

        // Handle the message
        let response = handle_message(message, &executor).await;

        // Send the response
        write_message(writer, &response).await?;

        // If we sent a Result or Error, the exchange is complete
        // Wait for another Execute message
    }
}

/// Handle a single message and produce a response.
async fn handle_message(message: Message, executor: &Arc<Mutex<Executor>>) -> Message {
    match message {
        Message::Execute { function, args } => handle_execute(function, args, executor).await,

        Message::Provide { functions } => handle_provide(functions, executor).await,

        // These are responses, not requests - protocol error
        Message::NeedDeps { .. } | Message::Result { .. } | Message::Error { .. } => {
            Message::error(ErrorKind::ProtocolError, "unexpected message type from client")
        }
    }
}

/// Handle an Execute request.
async fn handle_execute(
    function_hex: String,
    args: Vec<Value>,
    executor: &Arc<Mutex<Executor>>,
) -> Message {
    // Parse the function hash
    let function_hash = match parse_hash(&function_hex) {
        Ok(h) => h,
        Err(e) => return Message::error(ErrorKind::ProtocolError, e),
    };

    let mut exec = executor.lock().await;

    // Check if we have the function
    if !exec.store().contains(&function_hash) {
        return Message::need_deps(&[function_hash]);
    }

    // Check for missing dependencies
    let missing = exec.store().missing_dependencies(&function_hash);
    if !missing.is_empty() {
        return Message::need_deps(&missing);
    }

    // Execute the function
    match exec.vm_mut().call(&function_hash, args) {
        Ok(value) => Message::result(value),
        Err(e) => Message::error(ErrorKind::RuntimeException, e.to_string()),
    }
}

/// Handle a Provide request (client sending functions).
async fn handle_provide(functions: Vec<PortableFunction>, executor: &Arc<Mutex<Executor>>) -> Message {
    let mut exec = executor.lock().await;

    // Convert and add each function
    for pf in functions {
        let func = match CompiledFunction::try_from(pf) {
            Ok(f) => f,
            Err(e) => {
                return Message::error(
                    ErrorKind::ProtocolError,
                    format!("invalid function: {e}"),
                );
            }
        };

        // Add to store and load into VM
        let hash = func.hash;
        exec.store_mut().add(func.clone());
        exec.vm_mut().load_function(func);

        // Check if this resolved any dependencies
        let _missing = exec.store().missing_dependencies(&hash);
    }

    // Acknowledge receipt (client will send Execute again)
    // We could optimize by remembering the pending Execute, but this is simpler
    Message::result(Value::Unit)
}

/// Parse a hex-encoded hash string.
fn parse_hash(hex_str: &str) -> Result<blake3::Hash, String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "invalid hash length: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(blake3::Hash::from_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{BytecodeBuilder, Opcode};

    fn make_const_function(value: f64) -> CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(value));
        builder.emit(Opcode::Return);
        builder.build(0, 0)
    }

    #[tokio::test]
    async fn test_execute_missing_function() {
        let executor = Arc::new(Mutex::new(Executor::new()));
        let hash = blake3::hash(b"nonexistent");

        let response = handle_execute(hash.to_hex().to_string(), vec![], &executor).await;

        match response {
            Message::NeedDeps { hashes } => {
                assert_eq!(hashes.len(), 1);
                assert_eq!(hashes[0], hash.to_hex().to_string());
            }
            other => panic!("expected NeedDeps, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_provide_and_execute() {
        let executor = Arc::new(Mutex::new(Executor::new()));

        // Create a function
        let func = make_const_function(42.0);
        let hash = func.hash;

        // Provide the function
        let portable = PortableFunction::from(&func);
        let response = handle_provide(vec![portable], &executor).await;

        // Should acknowledge
        match response {
            Message::Result { value } => assert_eq!(value, Value::Unit),
            other => panic!("expected Result, got {other:?}"),
        }

        // Now execute
        let response = handle_execute(hash.to_hex().to_string(), vec![], &executor).await;

        match response {
            Message::Result { value } => assert_eq!(value, Value::Number(42.0)),
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_full_session() {
        use tokio::io::duplex;

        let executor = Arc::new(Mutex::new(Executor::new()));

        // Create a function
        let func = make_const_function(99.0);
        let hash = func.hash;

        // Set up duplex streams
        let (mut client_read, mut server_write) = duplex(4096);
        let (mut server_read, mut client_write) = duplex(4096);

        // Spawn the server session handler
        let server_executor = Arc::clone(&executor);
        let server_handle = tokio::spawn(async move {
            handle_session(&mut server_read, &mut server_write, server_executor).await
        });

        // Client: send Execute for missing function
        write_message(&mut client_write, &Message::execute(hash, vec![]))
            .await
            .expect("write execute");

        // Client: read NeedDeps
        let response = read_message(&mut client_read)
            .await
            .expect("read response")
            .expect("message present");

        match response {
            Message::NeedDeps { hashes } => {
                assert_eq!(hashes.len(), 1);
            }
            other => panic!("expected NeedDeps, got {other:?}"),
        }

        // Client: send Provide
        let portable = PortableFunction::from(&func);
        write_message(
            &mut client_write,
            &Message::Provide {
                functions: vec![portable],
            },
        )
        .await
        .expect("write provide");

        // Client: read acknowledgement
        let response = read_message(&mut client_read)
            .await
            .expect("read response")
            .expect("message present");

        match response {
            Message::Result { value } => assert_eq!(value, Value::Unit),
            other => panic!("expected ack Result, got {other:?}"),
        }

        // Client: send Execute again
        write_message(&mut client_write, &Message::execute(hash, vec![]))
            .await
            .expect("write execute");

        // Client: read result
        let response = read_message(&mut client_read)
            .await
            .expect("read response")
            .expect("message present");

        match response {
            Message::Result { value } => assert_eq!(value, Value::Number(99.0)),
            other => panic!("expected Result, got {other:?}"),
        }

        // Close client connection
        drop(client_write);

        // Wait for server to finish
        server_handle.await.expect("server task").expect("server session");
    }
}
