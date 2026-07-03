//! Time ability - for time-related operations.

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

/// `Time.now()` -> current timestamp in milliseconds since the Unix epoch.
// Handlers match the `HostHandler` signature, so the `Result` stays even
// where a handler cannot fail.
#[allow(clippy::unnecessary_wraps)]
fn now(_ability: &SuspendedAbility) -> Result<Value, VmError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Precision loss is acceptable for timestamps (won't exceed 52 bits for centuries)
    #[allow(clippy::cast_precision_loss)]
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64);
    Ok(Value::Number(now))
}

/// `Time.wait(duration)` -> sleeps for the given number of milliseconds.
#[allow(clippy::unnecessary_wraps)]
fn wait(ability: &SuspendedAbility) -> Result<Value, VmError> {
    if let Some(Value::Number(ms)) = ability.args.first() {
        // Negative durations are clamped to 0
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms_u64 = if *ms < 0.0 { 0 } else { *ms as u64 };
        let duration = std::time::Duration::from_millis(ms_u64);
        std::thread::sleep(duration);
    }
    Ok(Value::Unit)
}

/// Register the Time ability handlers on a VM.
///
/// Provides `now()` for getting current timestamp and `wait(ms)` for
/// sleeping.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_time(vm: &mut Vm, ability: &AbilityInterface) {
    vm.register_host_handler(ability.id, require(ability, "now"), Box::new(now));
    vm.register_host_handler(ability.id, require(ability, "wait"), Box::new(wait));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_core::AbilityId;

    fn suspended(args: Vec<Value>) -> SuspendedAbility {
        SuspendedAbility {
            ability_id: AbilityId::from_bytes([2; 32]),
            method_id: 0,
            args,
        }
    }

    #[test]
    fn test_time_now_returns_positive_number() {
        let result = now(&suspended(vec![]));
        assert!(result.is_ok());

        if let Value::Number(ms) = result.unwrap() {
            // Should be a positive number (milliseconds since epoch)
            assert!(ms > 0.0);
            // Should be reasonably recent (after year 2020)
            assert!(ms > 1_577_836_800_000.0); // Jan 1, 2020
        } else {
            panic!("Expected Number value");
        }
    }

    #[test]
    fn test_time_wait_returns_unit() {
        // Wait for 1 millisecond
        let result = wait(&suspended(vec![Value::Number(1.0)]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }

    #[test]
    fn test_time_wait_handles_negative_duration() {
        // Negative duration should be treated as 0
        let result = wait(&suspended(vec![Value::Number(-100.0)]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }
}
