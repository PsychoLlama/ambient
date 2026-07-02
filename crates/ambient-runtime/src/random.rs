//! Random ability - for random number generation.

use std::sync::{Arc, OnceLock};

use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: get a random number between 0.0 and 1.0.
pub const METHOD_SEED: u16 = 0x0000;

/// Method: get a random number in a range.
pub const METHOD_IN_RANGE: u16 = 0x0001;

/// The Random ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
        MethodDescriptor {
            id: METHOD_SEED,
            name: "seed",
            signature: MethodSignature {
                param_count: 0,
                param_types: |_f| vec![],
                return_type: |f| f.number(),
            },
        },
        MethodDescriptor {
            id: METHOD_IN_RANGE,
            name: "in_range",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.number()],
                return_type: |f| f.number(),
            },
        },
    ]
}

/// The content-addressed identity of the Random ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(RandomAbility::NAME, &methods()))
}

/// Random ability marker.
pub const RANDOM: RandomAbility = RandomAbility;

/// Marker type for the Random ability.
#[derive(Clone, Copy)]
pub struct RandomAbility;

impl RandomAbility {
    /// Ability name.
    pub const NAME: &'static str = "Random";

    /// The content-addressed identity of the Random ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Random RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Random ability implementation combining type info and handlers.
#[derive(Default)]
pub struct RandomRuntimeAbility;

impl RandomRuntimeAbility {
    /// Create a new Random ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for RandomRuntimeAbility {
    fn name(&self) -> &'static str {
        "Random"
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
            name: RandomAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEED: AtomicU64 = AtomicU64::new(0);

        fn next_random() -> f64 {
            let mut state = SEED.load(Ordering::Relaxed);
            if state == 0 {
                use std::time::{SystemTime, UNIX_EPOCH};
                #[allow(clippy::cast_possible_truncation)]
                let time_seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x853c_49e6_748f_ea9b);
                state = time_seed;
                if state == 0 {
                    state = 0x853c_49e6_748f_ea9b;
                }
            }
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            SEED.store(state, Ordering::Relaxed);
            #[allow(clippy::cast_precision_loss)]
            let result = (state as f64) / (u64::MAX as f64);
            result
        }

        let seed =
            Box::new(|_ability: &SuspendedAbility| Ok(Value::Number(next_random()))) as HostHandler;

        let in_range = Box::new(|ability: &SuspendedAbility| {
            if let Some(Value::Record(fields)) = ability.args.first() {
                let start = fields
                    .get(&Arc::from("start"))
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                let end = fields
                    .get(&Arc::from("end"))
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(1.0);
                let random = next_random();
                Ok(Value::Number(start + random * (end - start)))
            } else {
                let upper = ability
                    .args
                    .first()
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(1.0);
                Ok(Value::Number(next_random() * upper))
            }
        }) as HostHandler;

        vec![(METHOD_SEED, seed), (METHOD_IN_RANGE, in_range)]
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
    fn test_random_ability_constants() {
        assert_eq!(METHOD_SEED, 0x0000);
        assert_eq!(METHOD_IN_RANGE, 0x0001);
        // Identity is stable across calls.
        assert_eq!(ability_id(), RandomAbility::ability_id());
    }

    #[test]
    fn test_random_runtime_ability_name() {
        let random = RandomRuntimeAbility::new();
        assert_eq!(random.name(), "Random");
        assert_eq!(random.ability_id(), ability_id());
    }

    #[test]
    fn test_random_descriptor_methods() {
        let random = RandomRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = random.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
        assert_eq!(descriptor.name, "Random");
        assert_eq!(descriptor.methods.len(), 2);

        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"seed"));
        assert!(method_names.contains(&"in_range"));
    }

    #[test]
    fn test_random_seed_returns_number_in_range() {
        let random = RandomRuntimeAbility::new();
        let handlers = random.handlers();

        let (_, seed_handler) = handlers.iter().find(|(id, _)| *id == METHOD_SEED).unwrap();

        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_SEED,
            args: vec![],
        };

        // Call multiple times to verify range
        for _ in 0..10 {
            let result = seed_handler(&ability);
            assert!(result.is_ok());

            if let Value::Number(n) = result.unwrap() {
                assert!((0.0..=1.0).contains(&n), "Expected 0 <= {n} <= 1");
            } else {
                panic!("Expected Number value");
            }
        }
    }

    #[test]
    fn test_random_in_range_with_number() {
        let random = RandomRuntimeAbility::new();
        let handlers = random.handlers();

        let (_, in_range_handler) = handlers
            .iter()
            .find(|(id, _)| *id == METHOD_IN_RANGE)
            .unwrap();

        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_IN_RANGE,
            args: vec![Value::Number(100.0)],
        };

        let result = in_range_handler(&ability);
        assert!(result.is_ok());

        if let Value::Number(n) = result.unwrap() {
            assert!((0.0..=100.0).contains(&n), "Expected 0 <= {n} <= 100");
        } else {
            panic!("Expected Number value");
        }
    }

    #[test]
    fn test_random_produces_different_values() {
        let random = RandomRuntimeAbility::new();
        let handlers = random.handlers();

        let (_, seed_handler) = handlers.iter().find(|(id, _)| *id == METHOD_SEED).unwrap();

        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_SEED,
            args: vec![],
        };

        let mut values = std::collections::HashSet::new();
        for _ in 0..100 {
            if let Ok(Value::Number(n)) = seed_handler(&ability) {
                values.insert(n.to_bits());
            }
        }

        // Should produce at least some different values
        assert!(
            values.len() > 1,
            "Expected random to produce different values"
        );
    }
}
