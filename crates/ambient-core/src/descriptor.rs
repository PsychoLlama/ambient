//! Ability descriptors and provider trait.
//!
//! This module defines the interface for describing abilities in a way that
//! can be used by the type system and compiler without depending on engine types.

use crate::{AbilityId, MethodId};

/// Describes the type signature of an ability method.
///
/// This uses a factory function to defer type construction, allowing
/// the core crate to describe types without depending on the engine's
/// type system directly.
#[derive(Clone)]
pub struct MethodSignature<T> {
    /// Number of parameters the method takes.
    pub param_count: usize,

    /// Factory function that creates parameter types.
    /// Called with a type factory to construct engine-specific types.
    pub param_types: fn(&dyn TypeFactory<T>) -> Vec<T>,

    /// Factory function that creates the return type.
    pub return_type: fn(&dyn TypeFactory<T>) -> T,
}

/// Factory trait for creating engine-specific types.
///
/// The engine implements this trait to allow ability descriptors to
/// create types without depending on the engine directly.
pub trait TypeFactory<T> {
    /// Create the Unit type.
    fn unit(&self) -> T;

    /// Create the Bool type.
    fn bool(&self) -> T;

    /// Create the Number type.
    fn number(&self) -> T;

    /// Create the String type.
    fn string(&self) -> T;

    /// Create the Bytes type.
    fn bytes(&self) -> T;

    /// Create the Never type (for expressions that don't return).
    fn never(&self) -> T;

    /// Create a fresh type variable (for polymorphic methods).
    fn type_var(&self) -> T;

    /// Create a List<T> type.
    fn list(&self, element: T) -> T;
}

/// Describes a single method on an ability.
#[derive(Clone)]
pub struct MethodDescriptor<T> {
    /// Method ID (unique within the ability).
    pub id: MethodId,

    /// Method name as it appears in source code.
    pub name: &'static str,

    /// Type signature of the method.
    pub signature: MethodSignature<T>,
}

impl<T> MethodDescriptor<T> {
    /// Create a new method descriptor.
    pub const fn new(
        id: MethodId,
        name: &'static str,
        param_count: usize,
        param_types: fn(&dyn TypeFactory<T>) -> Vec<T>,
        return_type: fn(&dyn TypeFactory<T>) -> T,
    ) -> Self {
        Self {
            id,
            name,
            signature: MethodSignature {
                param_count,
                param_types,
                return_type,
            },
        }
    }
}

/// Describes an ability that can be registered with the engine.
#[derive(Clone)]
pub struct AbilityDescriptor<T: 'static> {
    /// Ability ID (globally unique).
    pub id: AbilityId,

    /// Ability name as it appears in source code.
    pub name: &'static str,

    /// Methods provided by this ability.
    pub methods: &'static [MethodDescriptor<T>],
}

impl<T> AbilityDescriptor<T> {
    /// Create a new ability descriptor.
    pub const fn new(
        id: AbilityId,
        name: &'static str,
        methods: &'static [MethodDescriptor<T>],
    ) -> Self {
        Self { id, name, methods }
    }

    /// Look up a method by name.
    pub fn get_method(&self, name: &str) -> Option<&MethodDescriptor<T>> {
        self.methods.iter().find(|m| m.name == name)
    }

    /// Look up a method by ID.
    pub fn get_method_by_id(&self, id: MethodId) -> Option<&MethodDescriptor<T>> {
        self.methods.iter().find(|m| m.id == id)
    }
}

/// Trait for types that provide abilities to the engine.
///
/// Implement this trait to define a set of abilities that can be
/// registered with the engine for type checking and compilation.
pub trait AbilityProvider<T> {
    /// Get all ability descriptors provided by this provider.
    fn abilities(&self) -> &[AbilityDescriptor<T>];

    /// Look up an ability by name.
    fn get_ability(&self, name: &str) -> Option<&AbilityDescriptor<T>> {
        self.abilities().iter().find(|a| a.name == name)
    }

    /// Look up an ability by ID.
    fn get_ability_by_id(&self, id: AbilityId) -> Option<&AbilityDescriptor<T>> {
        self.abilities().iter().find(|a| a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestType(String);

    /// A distinct, recognizable AbilityId for tests.
    fn test_id(byte: u8) -> crate::AbilityId {
        crate::AbilityId::from_bytes([byte; 32])
    }

    struct TestTypeFactory;

    impl TypeFactory<TestType> for TestTypeFactory {
        fn unit(&self) -> TestType {
            TestType("unit".to_string())
        }
        fn bool(&self) -> TestType {
            TestType("bool".to_string())
        }
        fn number(&self) -> TestType {
            TestType("number".to_string())
        }
        fn string(&self) -> TestType {
            TestType("string".to_string())
        }
        fn bytes(&self) -> TestType {
            TestType("bytes".to_string())
        }
        fn never(&self) -> TestType {
            TestType("never".to_string())
        }
        fn type_var(&self) -> TestType {
            TestType("var".to_string())
        }
        fn list(&self, element: TestType) -> TestType {
            TestType(format!("list<{}>", element.0))
        }
    }

    static TEST_METHODS: [MethodDescriptor<TestType>; 2] = [
        MethodDescriptor::new(0, "method_a", 1, |f| vec![f.string()], |f| f.unit()),
        MethodDescriptor::new(
            1,
            "method_b",
            2,
            |f| vec![f.number(), f.bool()],
            |f| f.number(),
        ),
    ];

    #[test]
    fn test_method_descriptor_new() {
        let method = MethodDescriptor::<TestType>::new(
            42,
            "test_method",
            1,
            |f| vec![f.string()],
            |f| f.bool(),
        );

        assert_eq!(method.id, 42);
        assert_eq!(method.name, "test_method");
        assert_eq!(method.signature.param_count, 1);
    }

    #[test]
    fn test_method_signature_param_types() {
        let factory = TestTypeFactory;
        let method = &TEST_METHODS[1];

        let params = (method.signature.param_types)(&factory);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], TestType("number".to_string()));
        assert_eq!(params[1], TestType("bool".to_string()));
    }

    #[test]
    fn test_method_signature_return_type() {
        let factory = TestTypeFactory;
        let method = &TEST_METHODS[0];

        let ret = (method.signature.return_type)(&factory);
        assert_eq!(ret, TestType("unit".to_string()));
    }

    #[test]
    fn test_ability_descriptor_new() {
        let ability =
            AbilityDescriptor::<TestType>::new(test_id(0x12), "TestAbility", &TEST_METHODS);

        assert_eq!(ability.id, test_id(0x12));
        assert_eq!(ability.name, "TestAbility");
        assert_eq!(ability.methods.len(), 2);
    }

    #[test]
    fn test_ability_descriptor_get_method() {
        let ability = AbilityDescriptor::<TestType>::new(test_id(1), "TestAbility", &TEST_METHODS);

        let method_a = ability.get_method("method_a");
        assert!(method_a.is_some());
        assert_eq!(method_a.expect("should exist").id, 0);

        let method_b = ability.get_method("method_b");
        assert!(method_b.is_some());
        assert_eq!(method_b.expect("should exist").id, 1);

        let missing = ability.get_method("nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_ability_descriptor_get_method_by_id() {
        let ability = AbilityDescriptor::<TestType>::new(test_id(1), "TestAbility", &TEST_METHODS);

        let method_0 = ability.get_method_by_id(0);
        assert!(method_0.is_some());
        assert_eq!(method_0.expect("should exist").name, "method_a");

        let method_1 = ability.get_method_by_id(1);
        assert!(method_1.is_some());
        assert_eq!(method_1.expect("should exist").name, "method_b");

        let missing = ability.get_method_by_id(99);
        assert!(missing.is_none());
    }

    struct TestProvider {
        abilities: Vec<AbilityDescriptor<TestType>>,
    }

    impl AbilityProvider<TestType> for TestProvider {
        fn abilities(&self) -> &[AbilityDescriptor<TestType>] {
            &self.abilities
        }
    }

    #[test]
    fn test_ability_provider_get_ability() {
        let provider = TestProvider {
            abilities: vec![
                AbilityDescriptor::new(test_id(1), "Ability1", &[]),
                AbilityDescriptor::new(test_id(2), "Ability2", &[]),
            ],
        };

        let ability1 = provider.get_ability("Ability1");
        assert!(ability1.is_some());
        assert_eq!(ability1.expect("should exist").id, test_id(1));

        let ability2 = provider.get_ability("Ability2");
        assert!(ability2.is_some());
        assert_eq!(ability2.expect("should exist").id, test_id(2));

        let missing = provider.get_ability("NonExistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_ability_provider_get_ability_by_id() {
        let provider = TestProvider {
            abilities: vec![
                AbilityDescriptor::new(test_id(100), "AbilityA", &[]),
                AbilityDescriptor::new(test_id(200), "AbilityB", &[]),
            ],
        };

        let ability_100 = provider.get_ability_by_id(test_id(100));
        assert!(ability_100.is_some());
        assert_eq!(ability_100.expect("should exist").name, "AbilityA");

        let ability_200 = provider.get_ability_by_id(test_id(200));
        assert!(ability_200.is_some());
        assert_eq!(ability_200.expect("should exist").name, "AbilityB");

        let missing = provider.get_ability_by_id(test_id(255));
        assert!(missing.is_none());
    }

    #[test]
    fn test_type_factory_list() {
        let factory = TestTypeFactory;
        let list_of_numbers = factory.list(factory.number());
        assert_eq!(list_of_numbers, TestType("list<number>".to_string()));
    }
}
