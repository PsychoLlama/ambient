//! Exception ability - core error handling.
//!
//! The Exception ability is fundamental to the language's error handling
//! semantics. It provides the `throw` method for raising errors that can
//! be caught by handlers.

use crate::descriptor::{
    AbilityDescriptor, AbilityProvider, MethodDescriptor, MethodSignature, TypeFactory,
};
use crate::AbilityId;

/// Exception ability ID.
///
/// This uses the historical ID 0x0002 for backward compatibility with
/// existing bytecode.
pub const ABILITY_ID: AbilityId = 0x0002;

/// Method ID for `throw`.
pub const METHOD_THROW: u16 = 0x0000;

/// Exception ability descriptor.
///
/// Note: The actual type-aware descriptor is constructed by the engine
/// using `CoreAbilities::new()`. This constant provides the IDs and names.
pub const EXCEPTION: ExceptionAbility = ExceptionAbility;

/// Marker type for the Exception ability.
#[derive(Clone, Copy)]
pub struct ExceptionAbility;

impl ExceptionAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Method ID for throw.
    pub const METHOD_THROW: u16 = METHOD_THROW;

    /// Ability name.
    pub const NAME: &'static str = "Exception";

    /// Method name for throw.
    pub const METHOD_THROW_NAME: &'static str = "throw";
}

/// Provider for core abilities (Exception, and future Map/List/etc).
///
/// This is parameterized by the type system's Type representation,
/// allowing it to work with different type systems.
pub struct CoreAbilities<T: 'static> {
    abilities: Vec<AbilityDescriptor<T>>,
}

impl<T: Clone + 'static> CoreAbilities<T> {
    /// Create a new core abilities provider.
    ///
    /// The type factory is used to construct type signatures for methods.
    pub fn new(factory: &dyn TypeFactory<T>) -> Self {
        let exception = AbilityDescriptor {
            id: ABILITY_ID,
            name: ExceptionAbility::NAME,
            methods: Box::leak(Box::new([MethodDescriptor {
                id: METHOD_THROW,
                name: ExceptionAbility::METHOD_THROW_NAME,
                signature: MethodSignature {
                    param_count: 1,
                    param_types: |f| vec![f.string()], // Error message is a string
                    return_type: |f| f.never(),        // throw never returns
                },
            }])),
        };

        // Factory is used when getting types from signatures
        let _ = factory;

        Self {
            abilities: vec![exception],
        }
    }
}

impl<T: Clone + 'static> AbilityProvider<T> for CoreAbilities<T> {
    fn abilities(&self) -> &[AbilityDescriptor<T>] {
        &self.abilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Simple test type for testing
    #[derive(Clone, Debug, PartialEq)]
    enum TestType {
        Unit,
        Bool,
        Number,
        String,
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
    fn test_exception_ability_ids() {
        assert_eq!(ExceptionAbility::ABILITY_ID, 0x0002);
        assert_eq!(ExceptionAbility::METHOD_THROW, 0x0000);
    }

    #[test]
    fn test_core_abilities_provider() {
        let factory = TestTypeFactory::new();
        let core = CoreAbilities::new(&factory);

        assert_eq!(core.abilities().len(), 1);

        let exception = core.get_ability("Exception");
        assert!(exception.is_some());

        let exception = exception.unwrap();
        assert_eq!(exception.id, ABILITY_ID);
        assert_eq!(exception.name, "Exception");
        assert_eq!(exception.methods.len(), 1);

        let throw = exception.get_method("throw");
        assert!(throw.is_some());

        let throw = throw.unwrap();
        assert_eq!(throw.id, METHOD_THROW);
        assert_eq!(throw.signature.param_count, 1);
    }

    #[test]
    fn test_method_signature_types() {
        let factory = TestTypeFactory::new();
        let core = CoreAbilities::new(&factory);

        let exception = core.get_ability("Exception").unwrap();
        let throw = exception.get_method("throw").unwrap();

        let params = (throw.signature.param_types)(&factory);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], TestType::String);

        let ret = (throw.signature.return_type)(&factory);
        assert_eq!(ret, TestType::Never);
    }
}
