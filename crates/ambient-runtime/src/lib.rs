//! Runtime abilities for the Ambient language.
//!
//! This crate defines host-provided abilities that depend on the execution
//! environment. These abilities may not be available in all environments
//! (e.g., WASM targets may not support File operations).
//!
//! Core abilities that the language depends on (like Exception) are defined
//! in `ambient-core` instead.

pub mod async_ability;
pub mod console;
pub mod execute;
pub mod log;
pub mod network;
pub mod random;
pub mod time;

pub use async_ability::{AsyncAbility, AsyncRuntimeAbility, ASYNC};
pub use console::{ConsoleAbility, ConsoleRuntimeAbility, CONSOLE};
pub use execute::ExecuteRuntimeAbility;
pub use log::{LogAbility, LogRuntimeAbility, LOG};
pub use network::{NetworkAbility, NetworkRuntimeAbility, NETWORK};
pub use random::{RandomAbility, RandomRuntimeAbility, RANDOM};
pub use time::{TimeAbility, TimeRuntimeAbility, TIME};

// Re-export RuntimeAbility trait for convenience
pub use ambient_ability::RuntimeAbility;

use ambient_core::{AbilityDescriptor, AbilityProvider, TypeFactory};

/// Provider for runtime abilities (Console, Time, Random, Async, Log, Network, Execute).
///
/// This is parameterized by the type system's Type representation,
/// allowing it to work with different type systems.
pub struct RuntimeAbilities<T: 'static> {
    abilities: Vec<AbilityDescriptor<T>>,
}

impl<T: Clone + 'static> RuntimeAbilities<T> {
    /// Create a new runtime abilities provider.
    ///
    /// The type factory is used to construct type signatures for methods.
    pub fn new(factory: &dyn TypeFactory<T>) -> Self {
        Self {
            abilities: vec![
                ConsoleRuntimeAbility::new().descriptor(factory),
                TimeRuntimeAbility::new().descriptor(factory),
                RandomRuntimeAbility::new().descriptor(factory),
                AsyncRuntimeAbility::new().descriptor(factory),
                LogRuntimeAbility::new().descriptor(factory),
                NetworkRuntimeAbility::new().descriptor(factory),
                ExecuteRuntimeAbility::new().descriptor(factory),
            ],
        }
    }
}

impl<T: Clone + 'static> AbilityProvider<T> for RuntimeAbilities<T> {
    fn abilities(&self) -> &[AbilityDescriptor<T>] {
        &self.abilities
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
    fn test_runtime_abilities_provider() {
        let factory = TestTypeFactory::new();
        let runtime = RuntimeAbilities::new(&factory);

        // 7 abilities: Console, Time, Random, Async, Log, Network, Execute
        assert_eq!(runtime.abilities().len(), 7);

        // Check Console
        let console = runtime.get_ability("Console");
        assert!(console.is_some());
        let console = console.unwrap();
        assert_eq!(console.methods.len(), 3);

        // Check Time
        let time = runtime.get_ability("Time");
        assert!(time.is_some());
        let time = time.unwrap();
        assert_eq!(time.methods.len(), 2);

        // Check Random
        let random = runtime.get_ability("Random");
        assert!(random.is_some());
        let random = random.unwrap();
        assert_eq!(random.methods.len(), 2);

        // Check Async
        let async_ab = runtime.get_ability("Async");
        assert!(async_ab.is_some());
        let async_ab = async_ab.unwrap();
        assert_eq!(async_ab.methods.len(), 2);

        // Check Log
        let log = runtime.get_ability("Log");
        assert!(log.is_some());
        let log = log.unwrap();
        assert_eq!(log.methods.len(), 4);

        // Check Network
        let network_ab = runtime.get_ability("Network");
        assert!(network_ab.is_some());
        let network_ab = network_ab.unwrap();
        assert_eq!(network_ab.methods.len(), 9);

        // Check Execute
        let execute_ab = runtime.get_ability("Execute");
        assert!(execute_ab.is_some());
        let execute_ab = execute_ab.unwrap();
        assert_eq!(execute_ab.methods.len(), 4);
    }

    #[test]
    fn test_ability_ids() {
        // Verify the historical IDs are correct
        assert_eq!(console::ABILITY_ID, 0x0001);
        assert_eq!(time::ABILITY_ID, 0x0003);
        assert_eq!(random::ABILITY_ID, 0x0004);
        assert_eq!(async_ability::ABILITY_ID, 0x0005);
        assert_eq!(log::ABILITY_ID, 0x0006);
        // 0x0007 was Remote (removed)
        assert_eq!(network::ABILITY_ID, 0x0008);
        assert_eq!(execute::ABILITY_ID, 0x0009);
    }
}
