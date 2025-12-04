//! TCP client for remote function execution.
//!
//! The client connects to a remote server and requests function execution.
//! It provides any dependencies the server requests from its local store.
//!
//! # Example
//!
//! ```ignore
//! use ambient_engine::client::Client;
//! use ambient_engine::store::Store;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a client with a pre-populated store
//!     let store = Store::new();
//!     // ... populate store with functions ...
//!
//!     let mut client = Client::new(store);
//!
//!     // Connect and execute
//!     let result = client.execute("127.0.0.1:8080", function_hash, vec![]).await;
//! }
//! ```

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, ToSocketAddrs};

use crate::protocol::{read_message, write_message, ErrorValue, Message, ProtocolError};
use crate::store::{PortableFunction, Store};
use crate::value::Value;

/// Errors that can occur during client operations.
#[derive(Debug)]
pub enum ClientError {
    /// I/O error.
    Io(std::io::Error),
    /// Protocol error.
    Protocol(ProtocolError),
    /// Server returned an error.
    Remote(ErrorValue),
    /// Function not found in local store.
    MissingFunction(blake3::Hash),
    /// Unexpected server response.
    UnexpectedResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Protocol(e) => write!(f, "protocol error: {e}"),
            Self::Remote(e) => write!(f, "remote error: {}", e.message),
            Self::MissingFunction(h) => write!(f, "function not in local store: {}", h.to_hex()),
            Self::UnexpectedResponse(msg) => write!(f, "unexpected response: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Protocol(e) => Some(e),
            Self::Remote(_) | Self::MissingFunction(_) | Self::UnexpectedResponse(_) => None,
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ProtocolError> for ClientError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

/// A TCP client for remote function execution.
///
/// The client maintains a local store of functions and provides them to
/// the server on demand during execution.
pub struct Client {
    /// Local store of functions to provide to the server.
    store: Store,
}

impl Client {
    /// Create a new client with an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Store::new(),
        }
    }

    /// Create a client with a pre-populated store.
    #[must_use]
    pub fn with_store(store: Store) -> Self {
        Self { store }
    }

    /// Get a reference to the local store.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Get mutable access to the local store.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Execute a function on a remote server.
    ///
    /// Connects to the server, sends the execute request, and handles any
    /// dependency requests before returning the result.
    pub async fn execute(
        &self,
        addr: impl ToSocketAddrs,
        function: blake3::Hash,
        args: Vec<Value>,
    ) -> Result<Value, ClientError> {
        let stream = TcpStream::connect(addr).await?;
        let (mut reader, mut writer) = stream.into_split();
        self.execute_on(&mut reader, &mut writer, function, args)
            .await
    }

    /// Execute a function using existing read/write streams.
    ///
    /// This is useful for testing or when managing connections manually.
    pub async fn execute_on<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
        function: blake3::Hash,
        args: Vec<Value>,
    ) -> Result<Value, ClientError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        // Send initial Execute request
        write_message(writer, &Message::execute(function, args)).await?;

        // Handle the conversation
        loop {
            let response = read_message(reader).await?.ok_or_else(|| {
                ClientError::UnexpectedResponse("server closed connection".into())
            })?;

            match response {
                Message::Result { value } => return Ok(value),

                Message::Error { error } => return Err(ClientError::Remote(error)),

                Message::NeedDeps { hashes } => {
                    // Collect requested functions from local store
                    let functions = self.collect_functions(&hashes)?;

                    // Send them to the server
                    write_message(writer, &Message::Provide { functions }).await?;

                    // Read acknowledgement
                    let ack = read_message(reader).await?.ok_or_else(|| {
                        ClientError::UnexpectedResponse("server closed after Provide".into())
                    })?;

                    match ack {
                        Message::Result { value: Value::Unit } => {
                            // Server acknowledged, re-send Execute
                            write_message(writer, &Message::execute(function, vec![])).await?;
                        }
                        Message::Error { error } => return Err(ClientError::Remote(error)),
                        other => {
                            return Err(ClientError::UnexpectedResponse(format!(
                                "expected Result(Unit) ack, got {other:?}"
                            )));
                        }
                    }
                }

                // These are client->server messages, not valid responses
                Message::Execute { .. } | Message::Provide { .. } => {
                    return Err(ClientError::UnexpectedResponse(
                        "server sent client message type".into(),
                    ));
                }
            }
        }
    }

    /// Collect functions by hash from the local store.
    fn collect_functions(&self, hashes: &[String]) -> Result<Vec<PortableFunction>, ClientError> {
        let mut functions = Vec::with_capacity(hashes.len());

        for hash_hex in hashes {
            let hash = parse_hash(hash_hex).map_err(|e| {
                ClientError::UnexpectedResponse(format!("invalid hash from server: {e}"))
            })?;

            let func = self
                .store
                .get(&hash)
                .ok_or(ClientError::MissingFunction(hash))?;

            // Also collect any dependencies
            self.collect_with_deps(&hash, &mut functions)?;

            // Add the main function if not already added
            if !functions.iter().any(|f| {
                let fhash = parse_hash(&f.hash).ok();
                fhash == Some(func.hash)
            }) {
                functions.push(PortableFunction::from(func.as_ref()));
            }
        }

        Ok(functions)
    }

    /// Recursively collect a function and its dependencies.
    fn collect_with_deps(
        &self,
        hash: &blake3::Hash,
        collected: &mut Vec<PortableFunction>,
    ) -> Result<(), ClientError> {
        // Get the function
        let func = self
            .store
            .get(hash)
            .ok_or(ClientError::MissingFunction(*hash))?;

        // Check if already collected
        let hash_hex = hash.to_hex().to_string();
        if collected.iter().any(|f| f.hash == hash_hex) {
            return Ok(());
        }

        // Collect dependencies first
        for dep_hash in &func.dependencies {
            self.collect_with_deps(dep_hash, collected)?;
        }

        // Add this function
        collected.push(PortableFunction::from(func.as_ref()));

        Ok(())
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
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
    use crate::protocol::ErrorKind;
    use tokio::io::duplex;

    fn make_const_function(value: f64) -> crate::bytecode::CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(value));
        builder.emit(Opcode::Return);
        builder.build(0, 0)
    }

    #[tokio::test]
    async fn test_execute_success() {
        // Set up duplex streams
        let (mut client_read, mut server_write) = duplex(4096);
        let (mut server_read, mut client_write) = duplex(4096);

        let func = make_const_function(42.0);
        let hash = func.hash;

        // Create client with the function
        let mut client = Client::new();
        client.store_mut().add(func);

        // Spawn a mock server
        let server_handle = tokio::spawn(async move {
            // Read Execute
            let msg = read_message(&mut server_read).await.unwrap().unwrap();
            assert!(matches!(msg, Message::Execute { .. }));

            // Send Result directly (function already "exists" on server)
            write_message(&mut server_write, &Message::result(Value::Number(42.0)))
                .await
                .unwrap();
        });

        // Execute on client
        let result = client
            .execute_on(&mut client_read, &mut client_write, hash, vec![])
            .await
            .expect("execute should succeed");

        assert_eq!(result, Value::Number(42.0));
        server_handle.await.expect("server task");
    }

    #[tokio::test]
    async fn test_execute_with_deps() {
        let (mut client_read, mut server_write) = duplex(4096);
        let (mut server_read, mut client_write) = duplex(4096);

        let func = make_const_function(123.0);
        let hash = func.hash;

        // Create client with the function
        let mut client = Client::new();
        client.store_mut().add(func.clone());

        // Spawn a mock server that requests deps
        let hash_hex = hash.to_hex().to_string();
        let server_handle = tokio::spawn(async move {
            // Read initial Execute
            let msg = read_message(&mut server_read).await.unwrap().unwrap();
            assert!(matches!(msg, Message::Execute { .. }));

            // Request the function
            write_message(&mut server_write, &Message::need_deps(&[hash]))
                .await
                .unwrap();

            // Read Provide
            let msg = read_message(&mut server_read).await.unwrap().unwrap();
            match msg {
                Message::Provide { functions } => {
                    assert_eq!(functions.len(), 1);
                    assert_eq!(functions[0].hash, hash_hex);
                }
                other => panic!("expected Provide, got {other:?}"),
            }

            // Send acknowledgement
            write_message(&mut server_write, &Message::result(Value::Unit))
                .await
                .unwrap();

            // Read re-Execute
            let msg = read_message(&mut server_read).await.unwrap().unwrap();
            assert!(matches!(msg, Message::Execute { .. }));

            // Send final result
            write_message(&mut server_write, &Message::result(Value::Number(123.0)))
                .await
                .unwrap();
        });

        // Execute on client
        let result = client
            .execute_on(&mut client_read, &mut client_write, hash, vec![])
            .await
            .expect("execute should succeed");

        assert_eq!(result, Value::Number(123.0));
        server_handle.await.expect("server task");
    }

    #[tokio::test]
    async fn test_execute_remote_error() {
        let (mut client_read, mut server_write) = duplex(4096);
        let (mut server_read, mut client_write) = duplex(4096);

        let hash = blake3::hash(b"test");

        let client = Client::new();

        // Spawn a mock server that returns an error
        let server_handle = tokio::spawn(async move {
            let _msg = read_message(&mut server_read).await.unwrap().unwrap();

            write_message(
                &mut server_write,
                &Message::error(ErrorKind::RuntimeException, "something went wrong"),
            )
            .await
            .unwrap();
        });

        // Execute on client
        let result = client
            .execute_on(&mut client_read, &mut client_write, hash, vec![])
            .await;

        match result {
            Err(ClientError::Remote(e)) => {
                assert_eq!(e.kind, ErrorKind::RuntimeException);
                assert_eq!(e.message, "something went wrong");
            }
            other => panic!("expected Remote error, got {other:?}"),
        }

        server_handle.await.expect("server task");
    }

    #[tokio::test]
    async fn test_execute_missing_local_function() {
        let (mut client_read, mut server_write) = duplex(4096);
        let (mut server_read, mut client_write) = duplex(4096);

        let hash = blake3::hash(b"exists on server");
        let missing_hash = blake3::hash(b"missing locally");

        let client = Client::new(); // Empty store

        // Spawn a mock server that requests a dep we don't have
        let server_handle = tokio::spawn(async move {
            let _msg = read_message(&mut server_read).await.unwrap().unwrap();

            // Request a function the client doesn't have
            write_message(&mut server_write, &Message::need_deps(&[missing_hash]))
                .await
                .unwrap();
        });

        // Execute on client
        let result = client
            .execute_on(&mut client_read, &mut client_write, hash, vec![])
            .await;

        match result {
            Err(ClientError::MissingFunction(h)) => {
                assert_eq!(h, missing_hash);
            }
            other => panic!("expected MissingFunction error, got {other:?}"),
        }

        server_handle.await.expect("server task");
    }

    /// Integration test: real client and server over TCP.
    #[tokio::test]
    async fn test_client_server_integration() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Create a function
        let func = make_const_function(777.0);
        let hash = func.hash;

        // Bind to a random available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn server task that handles one connection
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = stream.into_split();
            crate::server::handle_session(
                &mut reader,
                &mut writer,
                Arc::new(Mutex::new(crate::remote::Executor::new())),
            )
            .await
            .unwrap();
        });

        // Create client with the function
        let mut client = Client::new();
        client.store_mut().add(func);

        // Execute
        let result = client.execute(addr, hash, vec![]).await;

        assert_eq!(result.unwrap(), Value::Number(777.0));

        // Client disconnects, server should exit cleanly
        drop(client);
        server_handle.await.expect("server task");
    }

    /// Integration test: function with dependencies.
    #[tokio::test]
    async fn test_client_server_with_dependencies() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Create helper function that returns a constant
        let helper = make_const_function(100.0);
        let helper_hash = helper.hash;

        // Create main function that calls helper and adds 1
        let mut builder = BytecodeBuilder::new();
        builder.emit_call(helper_hash, 0); // Call helper()
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let main_func = builder.build(0, 0);
        let main_hash = main_func.hash;

        // Bind to a random available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn server task
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = stream.into_split();
            crate::server::handle_session(
                &mut reader,
                &mut writer,
                Arc::new(Mutex::new(crate::remote::Executor::new())),
            )
            .await
            .unwrap();
        });

        // Create client with both functions
        let mut client = Client::new();
        client.store_mut().add(helper);
        client.store_mut().add(main_func);

        // Execute main function
        let result = client.execute(addr, main_hash, vec![]).await;

        // Should get 100 + 1 = 101
        assert_eq!(result.unwrap(), Value::Number(101.0));

        drop(client);
        server_handle.await.expect("server task");
    }
}
