//! Network ability for TCP client/server operations.
//!
//! This module provides general-purpose networking primitives with message-oriented I/O.
//! Unlike the Remote ability which bundles networking with remote execution semantics,
//! Network provides low-level socket operations that can be used for any protocol.
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
//! - `send(conn: ConnectionId, data: List<number>) -> ()` - Send message
//! - `receive(conn: ConnectionId) -> List<number>` - Receive message
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

use ambient_ability::{HostHandler, RuntimeAbility};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Ability ID for Network (next after Remote = 0x0007).
pub const ABILITY_ID: AbilityId = 0x0008;

// ═══════════════════════════════════════════════════════════════════════════════
// Method IDs
// ═══════════════════════════════════════════════════════════════════════════════

/// Method: listen(address: string) -> ListenerId
pub const METHOD_LISTEN: u16 = 0x0000;

/// Method: accept(listener: ListenerId) -> ConnectionId
pub const METHOD_ACCEPT: u16 = 0x0001;

/// Method: close_listener(listener: ListenerId) -> ()
pub const METHOD_CLOSE_LISTENER: u16 = 0x0002;

/// Method: connect(address: string) -> ConnectionId
pub const METHOD_CONNECT: u16 = 0x0003;

/// Method: close(conn: ConnectionId) -> ()
pub const METHOD_CLOSE: u16 = 0x0004;

/// Method: send(conn: ConnectionId, data: List<number>) -> ()
pub const METHOD_SEND: u16 = 0x0005;

/// Method: receive(conn: ConnectionId) -> List<number>
pub const METHOD_RECEIVE: u16 = 0x0006;

/// Method: local_addr(conn: ConnectionId) -> string
pub const METHOD_LOCAL_ADDR: u16 = 0x0007;

/// Method: peer_addr(conn: ConnectionId) -> string
pub const METHOD_PEER_ADDR: u16 = 0x0008;

// ═══════════════════════════════════════════════════════════════════════════════
// Marker Types
// ═══════════════════════════════════════════════════════════════════════════════

/// Marker struct for the Network ability.
#[derive(Clone, Copy, Debug)]
pub struct NetworkAbility;

impl NetworkAbility {
    /// The name of this ability as it appears in Ambient code.
    pub const NAME: &'static str = "Network";
}

/// Constant for use in other modules.
pub const NETWORK: NetworkAbility = NetworkAbility;

// ═══════════════════════════════════════════════════════════════════════════════
// Network RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════════

/// Network ability implementation providing type descriptors.
///
/// Note: Network handlers require runtime configuration (tokio handle) so this
/// only provides the descriptor. Use `register_network` in ambient-engine to
/// set up handlers.
#[derive(Default)]
pub struct NetworkRuntimeAbility;

impl NetworkRuntimeAbility {
    /// Create a new Network ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for NetworkRuntimeAbility {
    fn name(&self) -> &'static str {
        "Network"
    }

    fn ability_id(&self) -> AbilityId {
        ABILITY_ID
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ABILITY_ID,
            name: "Network",
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: METHOD_LISTEN,
                    name: "listen",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_ACCEPT,
                    name: "accept",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_CLOSE_LISTENER,
                    name: "close_listener",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_CONNECT,
                    name: "connect",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_CLOSE,
                    name: "close",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_SEND,
                    name: "send",
                    signature: MethodSignature {
                        param_count: 2,
                        param_types: |f| vec![f.number(), f.list(f.number())],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_RECEIVE,
                    name: "receive",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.list(f.number()),
                    },
                },
                MethodDescriptor {
                    id: METHOD_LOCAL_ADDR,
                    name: "local_addr",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.string(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_PEER_ADDR,
                    name: "peer_addr",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.string(),
                    },
                },
            ])),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Network handlers require runtime configuration, so we can't provide default handlers.
        // Use register_network() in ambient-engine to set up handlers.
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestType;

    struct TestTypeFactory;

    impl TypeFactory<TestType> for TestTypeFactory {
        fn unit(&self) -> TestType {
            TestType
        }
        fn bool(&self) -> TestType {
            TestType
        }
        fn number(&self) -> TestType {
            TestType
        }
        fn string(&self) -> TestType {
            TestType
        }
        fn never(&self) -> TestType {
            TestType
        }
        fn type_var(&self) -> TestType {
            TestType
        }
        fn list(&self, _: TestType) -> TestType {
            TestType
        }
    }

    #[test]
    fn test_network_ability_constants() {
        assert_eq!(ABILITY_ID, 0x0008);
        assert_eq!(METHOD_LISTEN, 0x0000);
        assert_eq!(METHOD_ACCEPT, 0x0001);
        assert_eq!(METHOD_CLOSE_LISTENER, 0x0002);
        assert_eq!(METHOD_CONNECT, 0x0003);
        assert_eq!(METHOD_CLOSE, 0x0004);
        assert_eq!(METHOD_SEND, 0x0005);
        assert_eq!(METHOD_RECEIVE, 0x0006);
        assert_eq!(METHOD_LOCAL_ADDR, 0x0007);
        assert_eq!(METHOD_PEER_ADDR, 0x0008);
    }

    #[test]
    fn test_network_runtime_ability_name() {
        let network = NetworkRuntimeAbility::new();
        assert_eq!(network.name(), "Network");
        assert_eq!(network.ability_id(), ABILITY_ID);
    }

    #[test]
    fn test_network_descriptor_methods() {
        let network = NetworkRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = network.descriptor(&factory);

        assert_eq!(descriptor.id, ABILITY_ID);
        assert_eq!(descriptor.name, "Network");
        assert_eq!(descriptor.methods.len(), 9);

        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"listen"));
        assert!(method_names.contains(&"accept"));
        assert!(method_names.contains(&"close_listener"));
        assert!(method_names.contains(&"connect"));
        assert!(method_names.contains(&"close"));
        assert!(method_names.contains(&"send"));
        assert!(method_names.contains(&"receive"));
        assert!(method_names.contains(&"local_addr"));
        assert!(method_names.contains(&"peer_addr"));
    }

    #[test]
    fn test_network_handlers_empty() {
        // Network handlers require runtime configuration
        let network = NetworkRuntimeAbility::new();
        let handlers = network.handlers();
        assert!(handlers.is_empty());
    }
}
