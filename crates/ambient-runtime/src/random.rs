//! Random ability - for random number generation.

use std::sync::Arc;

use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Random ability ID.
///
/// This uses the historical ID 0x0004 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0004;

/// Method: get a random number between 0.0 and 1.0.
pub const METHOD_SEED: u16 = 0x0000;

/// Method: get a random number in a range.
pub const METHOD_IN_RANGE: u16 = 0x0001;

/// Random ability marker.
pub const RANDOM: RandomAbility = RandomAbility;

/// Marker type for the Random ability.
#[derive(Clone, Copy)]
pub struct RandomAbility;

impl RandomAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Random";
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
        ABILITY_ID
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ABILITY_ID,
            name: "Random",
            methods: Box::leak(Box::new([
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
            ])),
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
