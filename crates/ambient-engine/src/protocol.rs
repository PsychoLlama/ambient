//! Wire protocol for remote execution over the network.
//!
//! This module defines the message format for client-server communication.
//! Messages are length-prefixed JSON for simplicity and debuggability.
//!
//! # Wire Format
//!
//! Each message is sent as:
//! ```text
//! [4 bytes: length (big-endian u32)] [length bytes: JSON payload]
//! ```
//!
//! # Protocol Flow
//!
//! ```text
//! Client                          Server
//!   |                               |
//!   |-- Execute(hash, args) ------->|
//!   |                               |
//!   |<-- NeedDeps([hash1, hash2]) --|  (if server missing dependencies)
//!   |                               |
//!   |-- Provide([fn1, fn2]) ------->|
//!   |                               |
//!   |<-- Result(value) -------------|  (or Error)
//! ```

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::store::PortableFunction;
use crate::value::Value;

/// Maximum message size (16 MB).
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// A message in the wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    /// Request to execute a function (or closure).
    Execute {
        /// Hash of the function to execute (hex-encoded).
        function: String,
        /// Arguments to pass to the function.
        args: Vec<Value>,
        /// Captured environment for closures.
        #[serde(default)]
        captures: Vec<Value>,
        /// Required abilities for execution (e.g., `["Console", "Time"]`).
        ///
        /// The server checks these against its available abilities and returns
        /// an error if any are missing. This allows early failure before
        /// sending bytecode.
        #[serde(default)]
        required_abilities: Vec<String>,
    },

    /// Server requests missing dependencies.
    NeedDeps {
        /// Hashes of required functions (hex-encoded).
        hashes: Vec<String>,
    },

    /// Client provides requested functions.
    Provide {
        /// The functions being provided.
        functions: Vec<PortableFunction>,
    },

    /// Successful execution result.
    Result {
        /// The return value.
        value: Value,
    },

    /// Execution error.
    Error {
        /// The error details.
        error: ErrorValue,
    },
}

/// Error information with context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorValue {
    /// The kind of error.
    pub kind: ErrorKind,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional context (e.g., the value that caused the error).
    pub context: Option<Value>,
}

/// Categories of errors that can occur during remote execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    /// A required dependency is missing and cannot be resolved.
    MissingDependency,
    /// An ability was performed but no handler is available.
    AbilityNotProvided,
    /// A runtime exception occurred during execution.
    RuntimeException,
    /// Type mismatch (e.g., wrong argument types).
    TypeMismatch,
    /// The operation was cancelled.
    Cancelled,
    /// Protocol error (malformed message, etc.).
    ProtocolError,
    /// I/O error during communication.
    IoError,
}

/// Errors that can occur during protocol operations.
#[derive(Debug)]
pub enum ProtocolError {
    /// I/O error.
    Io(std::io::Error),
    /// JSON serialization/deserialization error.
    Json(serde_json::Error),
    /// Message too large.
    MessageTooLarge(u32),
    /// Connection closed unexpectedly.
    ConnectionClosed,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::MessageTooLarge(size) => {
                write!(
                    f,
                    "message too large: {size} bytes (max {MAX_MESSAGE_SIZE})"
                )
            }
            Self::ConnectionClosed => write!(f, "connection closed unexpectedly"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ProtocolError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Message I/O
// ─────────────────────────────────────────────────────────────────────────────

/// Write a message to an async writer.
///
/// Messages are length-prefixed: 4 bytes (big-endian u32) followed by JSON payload.
///
/// # Errors
///
/// Returns an error if:
/// - The message is too large (exceeds `MAX_MESSAGE_SIZE`)
/// - JSON serialization fails
/// - Writing to the stream fails
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Message,
) -> Result<(), ProtocolError> {
    let json = serde_json::to_vec(message)?;
    #[allow(clippy::cast_possible_truncation)]
    let len = json.len() as u32; // Safe: checked against MAX_MESSAGE_SIZE below

    if len > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::MessageTooLarge(len));
    }

    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;

    Ok(())
}

/// Read a message from an async reader.
///
/// Returns `None` if the connection is closed cleanly (EOF on length read).
///
/// # Errors
///
/// Returns an error if:
/// - The message is too large (exceeds `MAX_MESSAGE_SIZE`)
/// - JSON deserialization fails
/// - Reading from the stream fails
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Message>, ProtocolError> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf);

    if len > MAX_MESSAGE_SIZE {
        return Err(ProtocolError::MessageTooLarge(len));
    }

    // Read JSON payload
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;

    let message = serde_json::from_slice(&buf)?;
    Ok(Some(message))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

impl Message {
    /// Create an Execute message for a closure with required abilities.
    #[must_use]
    pub fn execute_closure_with_abilities(
        function_hash: blake3::Hash,
        args: Vec<Value>,
        captures: Vec<Value>,
        required_abilities: Vec<String>,
    ) -> Self {
        Self::Execute {
            function: function_hash.to_hex().to_string(),
            args,
            captures,
            required_abilities,
        }
    }

    /// Create an Execute message for a closure (no ability requirements).
    #[must_use]
    pub fn execute_closure(
        function_hash: blake3::Hash,
        args: Vec<Value>,
        captures: Vec<Value>,
    ) -> Self {
        Self::execute_closure_with_abilities(function_hash, args, captures, Vec::new())
    }

    /// Create an Execute message for a regular function with required abilities.
    #[must_use]
    pub fn execute_with_abilities(
        function_hash: blake3::Hash,
        args: Vec<Value>,
        required_abilities: Vec<String>,
    ) -> Self {
        Self::execute_closure_with_abilities(function_hash, args, Vec::new(), required_abilities)
    }

    /// Create an Execute message for a regular function (no captures or ability requirements).
    #[must_use]
    pub fn execute(function_hash: blake3::Hash, args: Vec<Value>) -> Self {
        Self::execute_closure(function_hash, args, Vec::new())
    }

    /// Create a `NeedDeps` message.
    #[must_use]
    pub fn need_deps(hashes: &[blake3::Hash]) -> Self {
        Self::NeedDeps {
            hashes: hashes.iter().map(|h| h.to_hex().to_string()).collect(),
        }
    }

    /// Create a Result message.
    #[must_use]
    pub fn result(value: Value) -> Self {
        Self::Result { value }
    }

    /// Create an Error message.
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Error {
            error: ErrorValue {
                kind,
                message: message.into(),
                context: None,
            },
        }
    }

    /// Create an Error message with context.
    pub fn error_with_context(kind: ErrorKind, message: impl Into<String>, context: Value) -> Self {
        Self::Error {
            error: ErrorValue {
                kind,
                message: message.into(),
                context: Some(context),
            },
        }
    }
}

impl ErrorValue {
    /// Create a new error value.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            context: None,
        }
    }

    /// Create an error value with context.
    pub fn with_context(kind: ErrorKind, message: impl Into<String>, context: Value) -> Self {
        Self {
            kind,
            message: message.into(),
            context: Some(context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let hash = blake3::hash(b"test::function");
        let msg = Message::execute(hash, vec![Value::Number(42.0)]);

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        match parsed {
            Message::Execute {
                function,
                args,
                required_abilities,
                ..
            } => {
                assert_eq!(function, hash.to_hex().to_string());
                assert_eq!(args.len(), 1);
                assert!(required_abilities.is_empty());
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_execute_with_abilities() {
        let hash = blake3::hash(b"test::function");
        let abilities = vec!["Console".to_string(), "Time".to_string()];
        let msg = Message::execute_with_abilities(hash, vec![], abilities.clone());

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        match parsed {
            Message::Execute {
                required_abilities, ..
            } => {
                assert_eq!(required_abilities, abilities);
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_error_serialization() {
        let msg = Message::error(ErrorKind::RuntimeException, "division by zero");

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        match parsed {
            Message::Error { error } => {
                assert_eq!(error.kind, ErrorKind::RuntimeException);
                assert_eq!(error.message, "division by zero");
                assert!(error.context.is_none());
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_error_with_context() {
        let msg = Message::error_with_context(
            ErrorKind::TypeMismatch,
            "expected number",
            Value::Bool(true),
        );

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        match parsed {
            Message::Error { error } => {
                assert_eq!(error.kind, ErrorKind::TypeMismatch);
                assert_eq!(error.context, Some(Value::Bool(true)));
            }
            _ => panic!("wrong message type"),
        }
    }

    #[tokio::test]
    async fn test_message_roundtrip() {
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(1024);

        let original = Message::execute(blake3::hash(b"test"), vec![Value::Number(1.0)]);

        write_message(&mut client, &original).await.unwrap();
        drop(client); // Close write side

        let received = read_message(&mut server).await.unwrap();
        assert!(received.is_some());

        match received.unwrap() {
            Message::Execute { args, .. } => {
                assert_eq!(args, vec![Value::Number(1.0)]);
            }
            _ => panic!("wrong message type"),
        }
    }

    #[tokio::test]
    async fn test_connection_closed() {
        use tokio::io::duplex;

        let (client, mut server) = duplex(1024);
        drop(client); // Close immediately

        let result = read_message(&mut server).await.unwrap();
        assert!(result.is_none()); // Clean EOF
    }
}
