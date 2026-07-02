//! Runtime abilities for the Ambient language.
//!
//! This crate defines host-provided abilities that depend on the execution
//! environment. These abilities may not be available in all environments
//! (e.g., WASM targets may not support File operations).
//!
//! Core abilities that the language depends on (like Exception) are defined
//! in `ambient-core` instead.

pub mod console;
pub mod execute;
pub mod fs;
pub mod log;
pub mod network;
pub mod random;
pub mod time;

/// The runtime bindings interface, as in-language `ability` declarations.
///
/// This source is the portable description of the native runtime: an
/// embedder parses it, resolves the declarations to content-addressed
/// identities, registers them as the `runtime` ability prelude for type
/// checking and compilation, and binds host handlers against the same
/// identities. The Rust descriptors in the sibling modules hash
/// identically (asserted by test) until they are retired.
pub const ABILITY_DECLARATIONS: &str = include_str!("runtime.ab");

pub use console::{ConsoleAbility, ConsoleRuntimeAbility, CONSOLE};
pub use execute::ExecuteRuntimeAbility;
pub use fs::{FsAbility, FsRuntimeAbility, FS};
pub use log::{LogAbility, LogRuntimeAbility, LOG};
pub use network::{NetworkAbility, NetworkRuntimeAbility, NETWORK};
pub use random::{RandomAbility, RandomRuntimeAbility, RANDOM};
pub use time::{TimeAbility, TimeRuntimeAbility, TIME};

// Re-export RuntimeAbility trait for convenience
pub use ambient_ability::RuntimeAbility;

use ambient_core::{AbilityDescriptor, AbilityProvider, TypeFactory};

/// Provider for runtime abilities (Console, Time, Random, Log, Fs, Network, Execute).
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
                LogRuntimeAbility::new().descriptor(factory),
                FsRuntimeAbility::new().descriptor(factory),
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
    fn test_runtime_abilities_provider() {
        let factory = TestTypeFactory::new();
        let runtime = RuntimeAbilities::new(&factory);

        // 7 abilities: Console, Time, Random, Log, Fs, Network, Execute
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

        // Check Log
        let log = runtime.get_ability("Log");
        assert!(log.is_some());
        let log = log.unwrap();
        assert_eq!(log.methods.len(), 4);

        // Check Fs
        let fs_ab = runtime.get_ability("Fs");
        assert!(fs_ab.is_some());
        let fs_ab = fs_ab.unwrap();
        assert_eq!(fs_ab.methods.len(), 8);

        // Check Network
        let network_ab = runtime.get_ability("Network");
        assert!(network_ab.is_some());
        let network_ab = network_ab.unwrap();
        assert_eq!(network_ab.methods.len(), 9);

        // Check Execute
        let execute_ab = runtime.get_ability("Execute");
        assert!(execute_ab.is_some());
        let execute_ab = execute_ab.unwrap();
        assert_eq!(execute_ab.methods.len(), 6);
    }

    #[test]
    fn test_ability_ids() {
        // Every ability's content-addressed identity must be distinct: the
        // interfaces differ, so the interface hashes must differ too.
        let ids = [
            ("Console", console::ability_id()),
            ("Time", time::ability_id()),
            ("Random", random::ability_id()),
            ("Log", log::ability_id()),
            ("Fs", fs::ability_id()),
            ("Network", network::ability_id()),
            ("Execute", execute::ability_id()),
            ("Exception", ambient_core::exception::ability_id()),
        ];

        for (i, (name_a, id_a)) in ids.iter().enumerate() {
            for (name_b, id_b) in &ids[i + 1..] {
                assert_ne!(
                    id_a, id_b,
                    "abilities {name_a} and {name_b} must not share an identity"
                );
            }
        }
    }
}
