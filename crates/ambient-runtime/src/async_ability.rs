//! Async ability - for concurrent execution of abilities.

use std::sync::OnceLock;

use ambient_ability::{HostHandler, RuntimeAbility};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: wait for all operations to complete.
/// Takes a list of suspended abilities, returns a list of results.
pub const METHOD_ALL: u16 = 0x0000;

/// Method: wait for first operation to complete, cancel others.
/// Takes a list of suspended abilities, returns the first result.
pub const METHOD_RACE: u16 = 0x0001;

/// The Async ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
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
    ]
}

/// The content-addressed identity of the Async ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(AsyncAbility::NAME, &methods()))
}

/// Async ability marker.
pub const ASYNC: AsyncAbility = AsyncAbility;

/// Marker type for the Async ability.
#[derive(Clone, Copy)]
pub struct AsyncAbility;

impl AsyncAbility {
    /// Ability name.
    pub const NAME: &'static str = "Async";

    /// The content-addressed identity of the Async ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
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
        ability_id()
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ability_id(),
            name: AsyncAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
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
        fn bytes(&self) -> TestType {
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
        assert_eq!(METHOD_ALL, 0x0000);
        assert_eq!(METHOD_RACE, 0x0001);
        // Identity is stable across calls.
        assert_eq!(ability_id(), AsyncAbility::ability_id());
    }

    #[test]
    fn test_async_runtime_ability_name() {
        let async_ability = AsyncRuntimeAbility::new();
        assert_eq!(async_ability.name(), "Async");
        assert_eq!(async_ability.ability_id(), ability_id());
    }

    #[test]
    fn test_async_descriptor_methods() {
        let async_ability = AsyncRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = async_ability.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
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
