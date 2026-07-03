//! Network ability for TCP client/server operations.
//!
//! This module provides general-purpose networking primitives with
//! message-oriented I/O: low-level socket operations that can be used for
//! any protocol.
//!
//! # API
//!
//! ## Server Operations
//! - `listen(address: string) -> ListenerId` - Bind TCP listener
//! - `accept(listener: ListenerId) -> ConnectionId` - Accept incoming connection
//! - `close_listener(listener: ListenerId) -> ()` - Stop accepting connections
//!
//! ## Client Operations
//! - `connect(address: string) -> ConnectionId` - Connect to remote server
//! - `close(conn: ConnectionId) -> ()` - Close connection
//!
//! ## Message I/O (length-prefixed)
//! - `send(conn: ConnectionId, data: Bytes) -> ()` - Send message
//! - `receive(conn: ConnectionId) -> Bytes` - Receive message
//!
//! ## Connection Info
//! - `local_addr(conn: ConnectionId) -> string` - Get local address
//! - `peer_addr(conn: ConnectionId) -> string` - Get peer address
//!
//! # Example
//!
//! Server:
//! ```ambient
//! let listener = Network.listen!("127.0.0.1:8080");
//! let conn = Network.accept!(listener);
//! let msg = Network.receive!(conn);
//! Network.send!(conn, process(msg));
//! Network.close!(conn);
//! ```
//!
//! Client:
//! ```ambient
//! let conn = Network.connect!("127.0.0.1:8080");
//! Network.send!(conn, my_request);
//! let response = Network.receive!(conn);
//! Network.close!(conn);
//! ```

use std::sync::Arc;

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;
use tokio::runtime::Handle as RuntimeHandle;

use crate::network_state::NetworkState;
use crate::{extract_bytes, extract_number, extract_string, require};

/// Configuration for the Network ability.
pub struct NetworkConfig {
    /// Tokio runtime handle for async operations.
    pub runtime: RuntimeHandle,
}

/// Register the Network ability handlers on a VM with private state.
///
/// Provides low-level TCP networking operations:
/// - `listen(address)` - Bind TCP listener
/// - `accept(listener)` - Accept connection
/// - `close_listener(listener)` - Close listener
/// - `connect(address)` - Connect to server
/// - `close(conn)` - Close connection
/// - `send(conn, data)` - Send length-prefixed message
/// - `receive(conn)` - Receive length-prefixed message
/// - `local_addr(conn)` - Get local address
/// - `peer_addr(conn)` - Get peer address
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_network(vm: &mut Vm, ability: &AbilityInterface, config: NetworkConfig) {
    let state = Arc::new(NetworkState::new(config.runtime));
    register_network_shared(vm, ability, state);
}

/// Register the Network ability handlers against shared state.
///
/// The process runtime registers every process VM against one
/// [`NetworkState`], so listener/connection handles are plain numbers
/// valid in any process — an acceptor can hand a connection to a
/// spawned worker by sending its handle in a message.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
pub fn register_network_shared(vm: &mut Vm, ability: &AbilityInterface, state: Arc<NetworkState>) {
    // Network.listen(address: string) -> ListenerId (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "listen"),
        Box::new(move |ability: &SuspendedAbility| {
            let addr = extract_string(&ability.args)?;
            let id = state_clone
                .listen(&addr)
                .map_err(|e| VmError::exception(format!("Network.listen: {e}")))?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Network.accept(listener: number) -> ConnectionId (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "accept"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let listener_id = extract_number(&ability.args)? as u64;
            let id = state_clone
                .accept(listener_id)
                .map_err(|e| VmError::exception(format!("Network.accept: {e}")))?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Network.close_listener(listener: number) -> ()
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "close_listener"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let listener_id = extract_number(&ability.args)? as u64;
            state_clone
                .close_listener(listener_id)
                .map_err(|e| VmError::exception(format!("Network.close_listener: {e}")))?;
            Ok(Value::Unit)
        }),
    );

    // Network.connect(address: string) -> ConnectionId (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "connect"),
        Box::new(move |ability: &SuspendedAbility| {
            let addr = extract_string(&ability.args)?;
            let id = state_clone
                .connect(&addr)
                .map_err(|e| VmError::exception(format!("Network.connect: {e}")))?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Network.close(conn: number) -> ()
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "close"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;
            state_clone
                .close(conn_id)
                .map_err(|e| VmError::exception(format!("Network.close: {e}")))?;
            Ok(Value::Unit)
        }),
    );

    // Network.send(conn: number, data: Bytes) -> ()
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "send"),
        Box::new(move |ability: &SuspendedAbility| {
            if ability.args.len() < 2 {
                return Err(VmError::TypeErrorOwned {
                    expected: "2 arguments".to_string(),
                    got: format!("{} arguments", ability.args.len()),
                });
            }

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = match &ability.args[0] {
                Value::Number(n) => *n as u64,
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "number".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };

            let data = extract_bytes(&ability.args[1])?;

            state_clone
                .send(conn_id, &data)
                .map_err(|e| VmError::exception(format!("Network.send: {e}")))?;
            Ok(Value::Unit)
        }),
    );

    // Network.receive(conn: number) -> Bytes
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "receive"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;

            let data = state_clone
                .receive(conn_id)
                .map_err(|e| VmError::exception(format!("Network.receive: {e}")))?;

            Ok(Value::bytes(data))
        }),
    );

    // Network.local_addr(conn: number) -> string
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "local_addr"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;

            let addr = state_clone
                .local_addr(conn_id)
                .map_err(|e| VmError::exception(format!("Network.local_addr: {e}")))?;
            Ok(Value::string(addr))
        }),
    );

    // Network.peer_addr(conn: number) -> string
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        ability.id,
        require(ability, "peer_addr"),
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;

            let addr = state_clone
                .peer_addr(conn_id)
                .map_err(|e| VmError::exception(format!("Network.peer_addr: {e}")))?;
            Ok(Value::string(addr))
        }),
    );
}
