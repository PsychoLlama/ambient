//! Random ability - for random number generation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

// Simple xorshift64 PRNG state - good enough for most purposes.
// Seeded from system time on first use.
static SEED: AtomicU64 = AtomicU64::new(0);

fn next_random() -> f64 {
    // Initialize seed if needed (using system time)
    let mut state = SEED.load(Ordering::Relaxed);
    if state == 0 {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Truncation is intentional - we only need 64 bits of entropy
        #[allow(clippy::cast_possible_truncation)]
        let time_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x853c_49e6_748f_ea9b);
        state = time_seed;
        if state == 0 {
            state = 0x853c_49e6_748f_ea9b; // fallback seed
        }
    }

    // xorshift64
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    SEED.store(state, Ordering::Relaxed);

    // Convert to 0.0-1.0 range
    // Cast precision loss is acceptable for random number generation
    #[allow(clippy::cast_precision_loss)]
    let result = (state as f64) / (u64::MAX as f64);
    result
}

/// `Random.seed()` -> a random number between 0.0 and 1.0.
// Handlers match the `HostHandler` signature, so the `Result` stays even
// where a handler cannot fail.
#[allow(clippy::unnecessary_wraps)]
fn seed(_ability: &SuspendedAbility) -> Result<Value, VmError> {
    Ok(Value::Number(next_random()))
}

/// `Random.in_range(range)` -> a random number in the given range.
///
/// The range is expected as a record `{ start: number, end: number }`;
/// a plain number is treated as an exclusive upper bound.
#[allow(clippy::unnecessary_wraps)]
fn in_range(ability: &SuspendedAbility) -> Result<Value, VmError> {
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
        // If not a record, treat as single number for upper bound
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
}

/// Register the Random ability handlers on a VM.
///
/// Provides `seed()` for random 0.0-1.0 and `in_range(range)` for random
/// in range.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_random(vm: &mut Vm, ability: &AbilityInterface) {
    vm.register_host_handler(ability.id, require(ability, "seed"), Box::new(seed));
    vm.register_host_handler(ability.id, require(ability, "in_range"), Box::new(in_range));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_core::AbilityId;

    fn suspended(args: Vec<Value>) -> SuspendedAbility {
        SuspendedAbility {
            ability_id: AbilityId::from_bytes([3; 32]),
            method_id: 0,
            args,
        }
    }

    #[test]
    fn test_random_seed_returns_number_in_range() {
        // Call multiple times to verify range
        for _ in 0..10 {
            let result = seed(&suspended(vec![]));
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
        let result = in_range(&suspended(vec![Value::Number(100.0)]));
        assert!(result.is_ok());

        if let Value::Number(n) = result.unwrap() {
            assert!((0.0..=100.0).contains(&n), "Expected 0 <= {n} <= 100");
        } else {
            panic!("Expected Number value");
        }
    }

    #[test]
    fn test_random_produces_different_values() {
        let mut values = std::collections::HashSet::new();
        for _ in 0..100 {
            if let Ok(Value::Number(n)) = seed(&suspended(vec![])) {
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
