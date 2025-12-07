//! Runtime abilities for the Ambient language.
//!
//! This crate defines host-provided abilities that depend on the execution
//! environment. These abilities may not be available in all environments
//! (e.g., WASM targets may not support File operations).
//!
//! Core abilities that the language depends on (like Exception) are defined
//! in `ambient-core` instead.

pub mod console;
pub mod time;
pub mod random;
pub mod async_ability;
pub mod log;

pub use console::{ConsoleAbility, CONSOLE};
pub use time::{TimeAbility, TIME};
pub use random::{RandomAbility, RANDOM};
pub use async_ability::{AsyncAbility, ASYNC};
pub use log::{LogAbility, LOG};

use ambient_core::{
    AbilityDescriptor, AbilityProvider, MethodDescriptor, MethodSignature, TypeFactory,
};

/// Provider for runtime abilities (Console, Time, Random, Async, Log).
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
    pub fn new(_factory: &dyn TypeFactory<T>) -> Self {
        let console_ability = AbilityDescriptor {
            id: console::ABILITY_ID,
            name: ConsoleAbility::NAME,
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: console::METHOD_PRINT,
                    name: "print",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: console::METHOD_PRINTLN,
                    name: "println",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: console::METHOD_EPRINT,
                    name: "eprint",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
            ])),
        };

        let time_ability = AbilityDescriptor {
            id: time::ABILITY_ID,
            name: TimeAbility::NAME,
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: time::METHOD_NOW,
                    name: "now",
                    signature: MethodSignature {
                        param_count: 0,
                        param_types: |_f| vec![],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: time::METHOD_WAIT,
                    name: "wait",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.unit(),
                    },
                },
            ])),
        };

        let random_ability = AbilityDescriptor {
            id: random::ABILITY_ID,
            name: RandomAbility::NAME,
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: random::METHOD_SEED,
                    name: "seed",
                    signature: MethodSignature {
                        param_count: 0,
                        param_types: |_f| vec![],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: random::METHOD_IN_RANGE,
                    name: "in_range",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()], // simplified - actually takes a record
                        return_type: |f| f.number(),
                    },
                },
            ])),
        };

        let async_ability = AbilityDescriptor {
            id: async_ability::ABILITY_ID,
            name: AsyncAbility::NAME,
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: async_ability::METHOD_ALL,
                    name: "all",
                    signature: MethodSignature {
                        param_count: 1,
                        // List<Ability<T, A!>> -> List<T>
                        // This is polymorphic - the actual types are inferred
                        param_types: |f| vec![f.type_var()],
                        return_type: |f| f.type_var(),
                    },
                },
                MethodDescriptor {
                    id: async_ability::METHOD_RACE,
                    name: "race",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.type_var()],
                        return_type: |f| f.type_var(),
                    },
                },
            ])),
        };

        let log_ability = AbilityDescriptor {
            id: log::ABILITY_ID,
            name: LogAbility::NAME,
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: log::METHOD_DEBUG,
                    name: "debug",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: log::METHOD_INFO,
                    name: "info",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: log::METHOD_WARN,
                    name: "warn",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: log::METHOD_ERROR,
                    name: "error",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
            ])),
        };

        Self {
            abilities: vec![
                console_ability,
                time_ability,
                random_ability,
                async_ability,
                log_ability,
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

        assert_eq!(runtime.abilities().len(), 5);

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
    }

    #[test]
    fn test_ability_ids() {
        // Verify the historical IDs are correct
        assert_eq!(console::ABILITY_ID, 0x0001);
        assert_eq!(time::ABILITY_ID, 0x0003);
        assert_eq!(random::ABILITY_ID, 0x0004);
        assert_eq!(async_ability::ABILITY_ID, 0x0005);
        assert_eq!(log::ABILITY_ID, 0x0006);
    }
}
