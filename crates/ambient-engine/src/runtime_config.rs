//! Runtime configuration for composable ability sets.
//!
//! This module provides `RuntimeConfig`, a structure for building custom
//! runtime ability sets that can be used with the VM and type checker.
//!
//! # Example
//!
//! ```ignore
//! // Full native runtime
//! let config = RuntimeConfig::native();
//!
//! // Custom runtime without Remote
//! let config = RuntimeConfig::native()
//!     .without_ability("Remote");
//!
//! // Create VM with config
//! let mut vm = Vm::with_runtime(&config);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};

use crate::runtime_ability::RuntimeAbility;
use crate::vm::HostHandler;

/// An entry in the runtime configuration.
struct AbilityEntry {
    name: Arc<str>,
    /// Factory function to create the ability implementation.
    /// Called each time we need descriptors or handlers.
    factory: Box<dyn Fn() -> Box<dyn RuntimeAbilityDyn> + Send + Sync>,
}

/// Dyn-compatible subset of `RuntimeAbility` for internal use.
trait RuntimeAbilityDyn: Send + Sync {
    fn ability_id(&self) -> AbilityId;
    fn handlers(&self) -> Vec<(MethodId, HostHandler)>;
}

impl<T: RuntimeAbility> RuntimeAbilityDyn for T {
    fn ability_id(&self) -> AbilityId {
        RuntimeAbility::ability_id(self)
    }
    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        RuntimeAbility::handlers(self)
    }
}

/// Configuration for runtime abilities.
///
/// Holds a set of abilities that provide both compile-time type information
/// and runtime handlers for the VM.
pub struct RuntimeConfig {
    abilities: HashMap<Arc<str>, AbilityEntry>,
}

impl RuntimeConfig {
    /// Create an empty runtime configuration with no abilities.
    #[must_use]
    pub fn new() -> Self {
        Self {
            abilities: HashMap::new(),
        }
    }

    /// Create the native runtime configuration with all standard abilities.
    ///
    /// Includes: Console, Time, Random, Async, Log.
    ///
    /// Note: Remote and Network abilities are not included because they require
    /// external dependencies (runtime handle, store). Use `register_remote()`
    /// and `register_network()` separately after creating the VM.
    #[must_use]
    pub fn native() -> Self {
        use crate::abilities::{
            AsyncRuntimeAbility, ConsoleRuntimeAbility, LogRuntimeAbility, NetworkRuntimeAbility,
            RandomRuntimeAbility, TimeRuntimeAbility,
        };

        Self::new()
            .with_ability_factory("Console", ConsoleRuntimeAbility::new)
            .with_ability_factory("Time", TimeRuntimeAbility::new)
            .with_ability_factory("Random", RandomRuntimeAbility::new)
            .with_ability_factory("Async", AsyncRuntimeAbility::new)
            .with_ability_factory("Log", LogRuntimeAbility::new)
            .with_ability_factory("Network", NetworkRuntimeAbility::new)
    }

    /// Add an ability using a factory function.
    ///
    /// The factory is called each time handlers or descriptors are needed.
    #[must_use]
    pub fn with_ability_factory<A, F>(mut self, name: &str, factory: F) -> Self
    where
        A: RuntimeAbility + 'static,
        F: Fn() -> A + Send + Sync + 'static,
    {
        self.abilities.insert(
            Arc::from(name),
            AbilityEntry {
                name: Arc::from(name),
                factory: Box::new(move || Box::new(factory())),
            },
        );
        self
    }

    /// Add an ability to the configuration.
    #[must_use]
    pub fn with_ability<A: RuntimeAbility + Clone + 'static>(self, ability: A) -> Self {
        let name = ability.name();
        self.with_ability_factory(name, move || ability.clone())
    }

    /// Remove an ability by name.
    #[must_use]
    pub fn without_ability(mut self, name: &str) -> Self {
        self.abilities.remove(name);
        self
    }

    /// Extend this configuration with abilities from another.
    ///
    /// Abilities from `other` override abilities with the same name in `self`.
    #[must_use]
    pub fn extend(mut self, other: Self) -> Self {
        self.abilities.extend(other.abilities);
        self
    }

    /// Get the names of all abilities in this configuration.
    #[must_use]
    pub fn ability_names(&self) -> Vec<&str> {
        self.abilities.keys().map(AsRef::as_ref).collect()
    }

    /// Check if this configuration contains an ability by name.
    #[must_use]
    pub fn has_ability(&self, name: &str) -> bool {
        self.abilities.contains_key(name)
    }

    /// Get ability descriptors for compilation/type checking.
    ///
    /// This creates temporary ability instances to get the descriptors.
    pub fn ability_descriptors<T: Clone + 'static>(
        &self,
        factory: &dyn TypeFactory<T>,
    ) -> Vec<AbilityDescriptor<T>> {
        use crate::abilities::{
            AsyncRuntimeAbility, ConsoleRuntimeAbility, LogRuntimeAbility, NetworkRuntimeAbility,
            RandomRuntimeAbility, TimeRuntimeAbility,
        };

        // We need to create descriptors for each ability type
        // Since we can't call the generic descriptor method through dyn trait,
        // we match on the ability name and create the appropriate descriptor
        self.abilities
            .values()
            .filter_map(|entry| {
                match entry.name.as_ref() {
                    "Console" => Some(ConsoleRuntimeAbility::new().descriptor(factory)),
                    "Time" => Some(TimeRuntimeAbility::new().descriptor(factory)),
                    "Random" => Some(RandomRuntimeAbility::new().descriptor(factory)),
                    "Async" => Some(AsyncRuntimeAbility::new().descriptor(factory)),
                    "Log" => Some(LogRuntimeAbility::new().descriptor(factory)),
                    "Network" => Some(NetworkRuntimeAbility::new().descriptor(factory)),
                    // Unknown abilities can't provide descriptors
                    _ => None,
                }
            })
            .collect()
    }

    /// Get all handlers for registering with a VM.
    ///
    /// Returns tuples of `(ability_id, method_id, handler)`.
    #[must_use]
    pub fn handlers(&self) -> Vec<(AbilityId, MethodId, HostHandler)> {
        let mut result = Vec::new();
        for entry in self.abilities.values() {
            let ability = (entry.factory)();
            let ability_id = ability.ability_id();
            for (method_id, handler) in ability.handlers() {
                result.push((ability_id, method_id, handler));
            }
        }
        result
    }

    /// Create from manifest ability strings.
    ///
    /// Currently supports:
    /// - `"ambient:native"` - All native abilities
    ///
    /// Unknown ability strings are ignored.
    #[must_use]
    pub fn from_manifest(abilities: &[String]) -> Self {
        let mut config = Self::new();
        for ability in abilities {
            if ability == "ambient:native" {
                config = config.extend(Self::native());
            }
            // Unknown abilities ignored for forward compatibility
        }
        config
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::new()
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
    fn test_empty_config() {
        let config = RuntimeConfig::new();
        assert!(config.ability_names().is_empty());
        assert!(config.handlers().is_empty());
    }

    #[test]
    fn test_native_config() {
        let config = RuntimeConfig::native();
        let names = config.ability_names();

        assert!(names.contains(&"Console"));
        assert!(names.contains(&"Time"));
        assert!(names.contains(&"Random"));
        assert!(names.contains(&"Async"));
        assert!(names.contains(&"Log"));
        assert!(names.contains(&"Network"));
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn test_without_ability() {
        let config = RuntimeConfig::native().without_ability("Console");
        let names = config.ability_names();

        assert!(!names.contains(&"Console"));
        assert!(names.contains(&"Time"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn test_ability_descriptors() {
        let config = RuntimeConfig::native();
        let factory = TestTypeFactory::new();
        let descriptors = config.ability_descriptors(&factory);

        assert_eq!(descriptors.len(), 6);

        let console = descriptors.iter().find(|d| d.name == "Console");
        assert!(console.is_some());
        assert_eq!(console.map(|d| d.methods.len()), Some(3)); // print, println, eprint

        let network = descriptors.iter().find(|d| d.name == "Network");
        assert!(network.is_some());
        assert_eq!(network.map(|d| d.methods.len()), Some(9)); // 9 network methods
    }

    #[test]
    fn test_handlers() {
        let config = RuntimeConfig::native();
        let handlers = config.handlers();

        // Console: 3, Time: 2, Random: 2, Async: 0 (VM opcodes), Log: 4, Network: 0 (registered separately)
        // Total: 11
        assert_eq!(handlers.len(), 11);
    }

    #[test]
    fn test_from_manifest() {
        let config = RuntimeConfig::from_manifest(&["ambient:native".to_string()]);
        assert_eq!(config.ability_names().len(), 6);

        let empty = RuntimeConfig::from_manifest(&[]);
        assert!(empty.ability_names().is_empty());
    }

    #[test]
    fn test_has_ability() {
        let config = RuntimeConfig::native();
        assert!(config.has_ability("Console"));
        assert!(config.has_ability("Time"));
        assert!(!config.has_ability("Unknown"));
    }
}
