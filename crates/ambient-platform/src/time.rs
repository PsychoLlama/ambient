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

/// `Time.wait(duration)` -> sleeps for the given `core::time::Duration`.
///
/// The argument is a `Duration` record — whole `secs` plus a subsecond
/// `nanos` remainder, mirroring the in-language type — not a bare
/// millisecond count. A missing or malformed record sleeps for zero.
#[allow(clippy::unnecessary_wraps)]
fn wait(ability: &SuspendedAbility) -> Result<Value, VmError> {
    if let Some(duration) = ability.args.first().and_then(duration_from_value) {
        std::thread::sleep(duration);
    }
    Ok(Value::Unit)
}

/// Decode a `core::time::Duration` value (a `{ secs, nanos }` record) into a
/// `std::time::Duration`.
///
/// The record is already normalized by the in-language constructors (`nanos`
/// in `[0, 1e9)`), so `secs`/`nanos` map straight onto `Duration::new`.
/// Negative or non-finite components describe a duration before the zero
/// point, which `thread::sleep` can't honor, so they clamp to zero.
fn duration_from_value(value: &Value) -> Option<std::time::Duration> {
    let Value::Record(fields) = value else {
        return None;
    };
    let field = |name: &str| match fields.get(name) {
        Some(Value::Number(n)) if n.is_finite() && *n >= 0.0 => *n,
        _ => 0.0,
    };
    // secs and nanos are whole numbers from the normalized record; the casts
    // saturate rather than wrap, so an absurd value simply sleeps a long time.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let secs = field("secs") as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let nanos = field("nanos") as u32;
    Some(std::time::Duration::new(secs, nanos))
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

    /// A `core::time::Duration` value, the way the in-language type stores it.
    fn duration(secs: f64, nanos: f64) -> Value {
        Value::record([
            ("secs", Value::Number(secs)),
            ("nanos", Value::Number(nanos)),
        ])
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
        // Wait for 1 millisecond (1e6 nanos).
        let result = wait(&suspended(vec![duration(0.0, 1_000_000.0)]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }

    #[test]
    fn test_time_wait_handles_negative_duration() {
        // A record with negative components describes a duration before the
        // zero point; it clamps to zero rather than erroring.
        let result = wait(&suspended(vec![duration(-1.0, -100.0)]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }

    #[test]
    fn test_time_wait_ignores_non_record_argument() {
        // A bare number is no longer a valid duration; treat it as zero-wait
        // instead of panicking.
        let result = wait(&suspended(vec![Value::Number(5.0)]));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }

    #[test]
    fn duration_from_value_reads_secs_and_nanos() {
        let got = duration_from_value(&duration(2.0, 500_000_000.0));
        assert_eq!(got, Some(std::time::Duration::new(2, 500_000_000)));
    }

    #[test]
    fn duration_from_value_rejects_non_record() {
        assert_eq!(duration_from_value(&Value::Number(1.0)), None);
    }
}
