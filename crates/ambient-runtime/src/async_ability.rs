//! Async ability - for concurrent execution of abilities.

use ambient_ability::{HostHandler, RuntimeAbility};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Async ability ID.
///
/// This uses the historical ID 0x0005 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0005;

/// Method: wait for all operations to complete.
/// Takes a list of suspended abilities, returns a list of results.
pub const METHOD_ALL: u16 = 0x0000;

/// Method: wait for first operation to complete, cancel others.
/// Takes a list of suspended abilities, returns the first result.
pub const METHOD_RACE: u16 = 0x0001;

/// Async ability marker.
pub const ASYNC: AsyncAbility = AsyncAbility;

/// Marker type for the Async ability.
#[derive(Clone, Copy)]
pub struct AsyncAbility;

impl AsyncAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Async";
}

// ═══════════════════════════════════════════════════════════════════════════
// Async RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Async ability implementation.
///
/// Note: `Async.all` and `Async.race` are handled by VM opcodes, not host handlers.
/// This provides only the type descriptor for compilation.
#[derive(Default)]
pub struct AsyncRuntimeAbility;

impl AsyncRuntimeAbility {
    /// Create a new Async ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for AsyncRuntimeAbility {
    fn name(&self) -> &'static str {
        "Async"
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
            name: "Async",
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: METHOD_ALL,
                    name: "all",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.type_var()],
                        return_type: |f| f.type_var(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_RACE,
                    name: "race",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.type_var()],
                        return_type: |f| f.type_var(),
                    },
                },
            ])),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Async is handled by VM opcodes (AsyncAll, AsyncRace), not host handlers
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
    fn test_async_ability_constants() {
        assert_eq!(ABILITY_ID, 0x0005);
        assert_eq!(METHOD_ALL, 0x0000);
        assert_eq!(METHOD_RACE, 0x0001);
    }

    #[test]
    fn test_async_runtime_ability_name() {
        let async_ability = AsyncRuntimeAbility::new();
        assert_eq!(async_ability.name(), "Async");
        assert_eq!(async_ability.ability_id(), ABILITY_ID);
    }

    #[test]
    fn test_async_descriptor_methods() {
        let async_ability = AsyncRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = async_ability.descriptor(&factory);

        assert_eq!(descriptor.id, ABILITY_ID);
        assert_eq!(descriptor.name, "Async");
        assert_eq!(descriptor.methods.len(), 2);

        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"all"));
        assert!(method_names.contains(&"race"));
    }

    #[test]
    fn test_async_handlers_empty() {
        // Async is handled by VM opcodes, not host handlers
        let async_ability = AsyncRuntimeAbility::new();
        let handlers = async_ability.handlers();
        assert!(handlers.is_empty());
    }
}
