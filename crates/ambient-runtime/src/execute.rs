//! Execute ability for server-side function execution.
//!
//! This ability enables a server to execute functions by their content-addressed
//! hash, supporting the remote execution protocol.
//!
//! # API
//!
//! - `has_function(hash: string) -> bool` - Check if function exists
//! - `get_dependencies(hash: string) -> List<string>` - Get function dependencies
//! - `load_functions(data: Bytes) -> ()` - Load portable functions
//! - `run<T, R>(hash: string, args: T) -> R` - Execute function by hash

use ambient_ability::{HostHandler, RuntimeAbility};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Execute ability ID (0x0009).
pub const ABILITY_ID: AbilityId = 0x0009;

// ═══════════════════════════════════════════════════════════════════════════════
// Method IDs
// ═══════════════════════════════════════════════════════════════════════════════

/// Method: has_function(hash: string) -> bool
pub const METHOD_HAS_FUNCTION: MethodId = 0x0000;

/// Method: get_dependencies(hash: string) -> List<string>
pub const METHOD_GET_DEPENDENCIES: MethodId = 0x0001;

/// Method: load_functions(data: List<number>) -> ()
pub const METHOD_LOAD_FUNCTIONS: MethodId = 0x0002;

/// Method: run<T, R>(hash: string, args: T) -> R
pub const METHOD_RUN: MethodId = 0x0003;

// ═══════════════════════════════════════════════════════════════════════════════
// Execute Ability Constant
// ═══════════════════════════════════════════════════════════════════════════════

/// Marker struct for the Execute ability.
pub struct ExecuteAbility;

impl ExecuteAbility {
    /// The name of this ability as it appears in Ambient code.
    pub const NAME: &'static str = "Execute";
}

/// Constant for use in other modules.
pub const EXECUTE: ExecuteAbility = ExecuteAbility;

// ═══════════════════════════════════════════════════════════════════════════════
// Execute RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════════

/// Execute ability implementation providing type descriptors.
///
/// Note: Execute handlers require runtime configuration (function store, VM)
/// so this only provides the descriptor. Use `register_execute` in ambient-engine
/// to set up handlers.
#[derive(Default, Clone)]
pub struct ExecuteRuntimeAbility;

impl ExecuteRuntimeAbility {
    /// Create a new Execute ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for ExecuteRuntimeAbility {
    fn name(&self) -> &'static str {
        "Execute"
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
            name: "Execute",
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: METHOD_HAS_FUNCTION,
                    name: "has_function",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.bool(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_GET_DEPENDENCIES,
                    name: "get_dependencies",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.list(f.string()),
                    },
                },
                MethodDescriptor {
                    id: METHOD_LOAD_FUNCTIONS,
                    name: "load_functions",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.bytes()], // serialized data
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_RUN,
                    name: "run",
                    signature: MethodSignature {
                        param_count: 2,
                        param_types: |f| vec![f.string(), f.type_var()], // hash, args
                        return_type: |f| f.type_var(),                   // result
                    },
                },
            ])),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Handlers are registered separately via register_execute()
        // since they need access to the function store and VM
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum TestType {
        Unit,
        Bool,
        Number,
        String,
        Bytes,
        Never,
        Var(u32),
        List(Box<TestType>),
    }

    struct TestTypeFactory {
        next_var: std::cell::Cell<u32>,
    }

    impl TestTypeFactory {
        fn new() -> Self {
            Self {
                next_var: std::cell::Cell::new(0),
            }
        }
    }

    impl TypeFactory<TestType> for TestTypeFactory {
        fn unit(&self) -> TestType {
            TestType::Unit
        }
        fn bool(&self) -> TestType {
            TestType::Bool
        }
        fn number(&self) -> TestType {
            TestType::Number
        }
        fn string(&self) -> TestType {
            TestType::String
        }
        fn bytes(&self) -> TestType {
            TestType::Bytes
        }
        fn never(&self) -> TestType {
            TestType::Never
        }
        fn type_var(&self) -> TestType {
            let id = self.next_var.get();
            self.next_var.set(id + 1);
            TestType::Var(id)
        }
        fn list(&self, element: TestType) -> TestType {
            TestType::List(Box::new(element))
        }
    }

    #[test]
    fn test_ability_id() {
        assert_eq!(ABILITY_ID, 0x0009);
    }

    #[test]
    fn test_ability_name() {
        let ability = ExecuteRuntimeAbility::new();
        assert_eq!(ability.name(), "Execute");
    }

    #[test]
    fn test_descriptor() {
        let ability = ExecuteRuntimeAbility::new();
        let factory = TestTypeFactory::new();
        let descriptor = ability.descriptor(&factory);

        assert_eq!(descriptor.id, 0x0009);
        assert_eq!(descriptor.name, "Execute");
        assert_eq!(descriptor.methods.len(), 4);

        // Check method names
        let names: Vec<&str> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(names.contains(&"has_function"));
        assert!(names.contains(&"get_dependencies"));
        assert!(names.contains(&"load_functions"));
        assert!(names.contains(&"run"));
    }

    #[test]
    fn test_handlers_empty() {
        let ability = ExecuteRuntimeAbility::new();
        // Handlers are registered separately
        assert!(ability.handlers().is_empty());
    }
}
