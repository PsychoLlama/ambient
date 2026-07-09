//! Network natives for TCP client/server operations.
//!
//! General-purpose networking primitives with message-oriented I/O:
//! low-level socket operations that can be used for any protocol.
//! Listener and connection handles are plain numbers, valid in any VM
//! registered against the same [`NetworkState`] — an acceptor can hand a
//! connection to a spawned worker by sending its handle in a message.
//!
//! # Errors
//!
//! Every method is fallible and returns an in-language `Result<T, String>`:
//! an operational failure (refused connection, closed handle, invalid
//! endpoint) is converted to a `Result::Err(message)` value by
//! [`crate::into_result`], while argument-type mismatches remain fatal type
//! errors. Each closure computes its natural `Result<Value, VmError>`
//! (bare value on success, `VmError::exception` on failure) and hands it to
//! `into_result` for wrapping.

use std::sync::Arc;

use ambient_ability::{Value, VmError};
use ambient_engine::natives::NativeRegistry;

use crate::network_state::NetworkState;
use crate::{bind, extract_bytes, extract_host_port, extract_number, into_result};

/// The `Network` native implementations, bound against shared state.
///
/// The process runtime hands every process VM the same [`NetworkState`],
/// so handles cross process boundaries freely.
#[must_use]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn network_natives(state: Arc<NetworkState>) -> NativeRegistry {
    let mut registry = NativeRegistry::new();

    // network_listen(endpoint: (string, number)) -> Result<ListenerId, string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_listen",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                let (host, port) = extract_host_port(&args)?;
                let id = state_clone
                    .listen(&host, port)
                    .map_err(|e| VmError::exception(format!("Network.listen: {e}")))?;
                #[allow(clippy::cast_precision_loss)]
                Ok(Value::Number(id as f64))
            })())
        }),
    );

    // network_accept(listener: number) -> Result<ConnectionId, string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_accept",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let listener_id = extract_number(&args)? as u64;
                let id = state_clone
                    .accept(listener_id)
                    .map_err(|e| VmError::exception(format!("Network.accept: {e}")))?;
                #[allow(clippy::cast_precision_loss)]
                Ok(Value::Number(id as f64))
            })())
        }),
    );

    // network_close_listener(listener: number) -> Result<(), string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_close_listener",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let listener_id = extract_number(&args)? as u64;
                state_clone
                    .close_listener(listener_id)
                    .map_err(|e| VmError::exception(format!("Network.close_listener: {e}")))?;
                Ok(Value::Unit)
            })())
        }),
    );

    // network_connect(endpoint: (string, number)) -> Result<ConnectionId, string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_connect",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                let (host, port) = extract_host_port(&args)?;
                let id = state_clone
                    .connect(&host, port)
                    .map_err(|e| VmError::exception(format!("Network.connect: {e}")))?;
                #[allow(clippy::cast_precision_loss)]
                Ok(Value::Number(id as f64))
            })())
        }),
    );

    // network_close(conn: number) -> Result<(), string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_close",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;
                state_clone
                    .close(conn_id)
                    .map_err(|e| VmError::exception(format!("Network.close: {e}")))?;
                Ok(Value::Unit)
            })())
        }),
    );

    // network_send(conn: number, data: Binary) -> Result<(), string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_send",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                if args.len() < 2 {
                    return Err(VmError::TypeErrorOwned {
                        expected: "2 arguments".to_string(),
                        got: format!("{} arguments", args.len()),
                    });
                }

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = match &args[0] {
                    Value::Number(n) => *n as u64,
                    other => {
                        return Err(VmError::TypeErrorOwned {
                            expected: "Number".to_string(),
                            got: other.type_name().to_string(),
                        });
                    }
                };

                let data = extract_bytes(&args[1])?;

                state_clone
                    .send(conn_id, &data)
                    .map_err(|e| VmError::exception(format!("Network.send: {e}")))?;
                Ok(Value::Unit)
            })())
        }),
    );

    // network_receive(conn: number) -> Result<Binary, string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_receive",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;

                let data = state_clone
                    .receive(conn_id)
                    .map_err(|e| VmError::exception(format!("Network.receive: {e}")))?;

                Ok(Value::binary(data))
            })())
        }),
    );

    // network_local_addr(conn: number) -> Result<string, string>
    let state_clone = Arc::clone(&state);
    bind(
        &mut registry,
        "network_local_addr",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;

                let addr = state_clone
                    .local_addr(conn_id)
                    .map_err(|e| VmError::exception(format!("Network.local_addr: {e}")))?;
                Ok(Value::string(addr))
            })())
        }),
    );

    // network_peer_addr(conn: number) -> Result<string, string>
    bind(
        &mut registry,
        "network_peer_addr",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;

                let addr = state
                    .peer_addr(conn_id)
                    .map_err(|e| VmError::exception(format!("Network.peer_addr: {e}")))?;
                Ok(Value::string(addr))
            })())
        }),
    );

    registry
}
