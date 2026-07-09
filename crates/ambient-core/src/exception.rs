//! Exception ability - core error handling.
//!
//! The Exception ability is fundamental to the language's error handling
//! semantics. It provides the `throw` method for raising errors that can
//! be caught by handlers.
//!
//! Exception is declared in Ambient source (`core::exception`, re-exported
//! from the prelude) like any other ability; it is not an engine builtin.
//! What lives here is only its *identity*: the content hash of its canonical
//! interface, which the VM's throw/unwind path keys on. The in-language
//! declaration reproduces this exact hash, so the two never drift.

use std::sync::OnceLock;

use crate::AbilityId;
use crate::canonical::hash_interface;
use crate::descriptor::MethodDescriptor;

/// Method ID for `throw`.
pub const METHOD_THROW: u16 = 0x0000;

/// The Exception ability's method set, instantiated for any type system.
///
/// This is the single source of truth for the interface: both the
/// content-addressed [`ability_id`] and the engine-facing descriptor are
/// derived from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![MethodDescriptor::new(
        METHOD_THROW,
        ExceptionAbility::METHOD_THROW_NAME,
        1,
        |f| vec![f.string()], // Error message is a string
        |f| f.never(),        // throw never returns
    )]
}

/// The content-addressed identity of the Exception ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(ExceptionAbility::NAME, &methods()))
}

/// Marker type for the Exception ability: a home for its identity
/// constants (name, method id, content-addressed [`ability_id`]).
#[derive(Clone, Copy)]
pub struct ExceptionAbility;

impl ExceptionAbility {
    /// Method ID for throw.
    pub const METHOD_THROW: u16 = METHOD_THROW;

    /// Ability name.
    pub const NAME: &'static str = "Exception";

    /// Method name for throw.
    pub const METHOD_THROW_NAME: &'static str = "throw";

    /// The content-addressed identity of the Exception ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::TypeFactory;

    // Simple test type for testing
    #[derive(Clone, Debug, PartialEq)]
    enum TestType {
        Unit,
        Bool,
        Number,
        String,
        Binary,
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
        fn binary(&self) -> TestType {
            TestType::Binary
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
    fn test_exception_ability_id_stable() {
        assert_eq!(ExceptionAbility::ability_id(), ability_id());
        assert_eq!(ExceptionAbility::METHOD_THROW, 0x0000);
    }

    /// The canonical interface the [`ability_id`] hash — and the
    /// in-language `core::exception::Exception` declaration that must
    /// reproduce it — commits to: a single method `throw(string): never`.
    #[test]
    fn test_exception_interface_shape() {
        let factory = TestTypeFactory::new();
        let methods = methods::<TestType>();

        assert_eq!(methods.len(), 1);
        let throw = &methods[0];
        assert_eq!(throw.id, METHOD_THROW);
        assert_eq!(throw.name, "throw");
        assert_eq!(throw.signature.param_count, 1);

        let params = (throw.signature.param_types)(&factory);
        assert_eq!(params, vec![TestType::String]);

        let ret = (throw.signature.return_type)(&factory);
        assert_eq!(ret, TestType::Never);
    }
}
