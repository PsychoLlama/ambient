//! Remote execution ability for distributed function calls.
//!
//! This module defines the Remote ability which enables sending functions
//! to other Ambient VM processes for execution.
//!
//! # API
//!
//! - `listen(address: string) -> Listener` - Bind TCP listener
//! - `accept(listener: Listener) -> Connection` - Accept one connection
//! - `connect(address: string) -> Connection` - Connect as client
//! - `call(conn: Connection, thunk: () -> T) -> T` - Send thunk for remote execution
//! - `serve(conn: Connection) -> value` - Wait for and execute one remote call
//! - `close(conn: Connection) -> ()` - Close connection
//!
//! # Example
//!
//! Server:
//! ```ambient
//! let listener = Remote.listen!("127.0.0.1:8080");
//! let conn = Remote.accept!(listener);
//! loop {
//!     let result = Remote.serve!(conn);
//! }
//! ```
//!
//! Client:
//! ```ambient
//! let conn = Remote.connect!("127.0.0.1:8080");
//! let result = Remote.call!(conn, () => my_function(arg));
//! Remote.close!(conn);
//! ```

use ambient_ability::{HostHandler, RuntimeAbility};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Ability ID for Remote (next after Log = 0x0006).
pub const ABILITY_ID: AbilityId = 0x0007;

/// Method: listen(address: string) -> Listener
pub const METHOD_LISTEN: u16 = 0x0000;

/// Method: accept(listener: Listener) -> Connection
pub const METHOD_ACCEPT: u16 = 0x0001;

/// Method: connect(address: string) -> Connection
pub const METHOD_CONNECT: u16 = 0x0002;

/// Method: call(conn: Connection, thunk: () -> T) -> T
pub const METHOD_CALL: u16 = 0x0003;

/// Method: close(conn: Connection) -> ()
pub const METHOD_CLOSE: u16 = 0x0004;

/// Method: serve(conn: Connection) -> value
pub const METHOD_SERVE: u16 = 0x0005;

/// Marker struct for the Remote ability.
#[derive(Clone, Copy, Debug)]
pub struct RemoteAbility;

impl RemoteAbility {
    /// The name of this ability as it appears in Ambient code.
    pub const NAME: &'static str = "Remote";
}

/// Constant for use in other modules.
pub const REMOTE: RemoteAbility = RemoteAbility;

// ═══════════════════════════════════════════════════════════════════════════
// Remote RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Remote ability implementation providing type descriptors.
///
/// Note: Remote handlers require runtime configuration (tokio handle, store)
/// so this only provides the descriptor. Use `register_remote` in ambient-engine
/// to set up handlers.
#[derive(Default)]
pub struct RemoteRuntimeAbility;

impl RemoteRuntimeAbility {
    /// Create a new Remote ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for RemoteRuntimeAbility {
    fn name(&self) -> &'static str {
        "Remote"
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
            name: "Remote",
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
                    id: METHOD_CONNECT,
                    name: "connect",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_CALL,
                    name: "call",
                    signature: MethodSignature {
                        param_count: 2,
                        param_types: |f| vec![f.number(), f.type_var()],
                        return_type: |f| f.type_var(),
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
                    id: METHOD_SERVE,
                    name: "serve",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.type_var(),
                    },
                },
            ])),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Remote handlers require runtime configuration, so we can't provide default handlers.
        // Use register_remote() in ambient-engine to set up handlers.
        vec![]
    }
}
