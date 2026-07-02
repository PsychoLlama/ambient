//! Time ability - for time-related operations.

use std::sync::OnceLock;

use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: get current timestamp in milliseconds.
pub const METHOD_NOW: u16 = 0x0000;

/// Method: wait for a duration in milliseconds.
pub const METHOD_WAIT: u16 = 0x0001;

/// The Time ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
        MethodDescriptor {
            id: METHOD_NOW,
            name: "now",
            signature: MethodSignature {
                param_count: 0,
                param_types: |_f| vec![],
                return_type: |f| f.number(),
            },
        },
        MethodDescriptor {
            id: METHOD_WAIT,
            name: "wait",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.number()],
                return_type: |f| f.unit(),
            },
        },
    ]
}

/// The content-addressed identity of the Time ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(TimeAbility::NAME, &methods()))
}

/// Time ability marker.
pub const TIME: TimeAbility = TimeAbility;

/// Marker type for the Time ability.
#[derive(Clone, Copy)]
pub struct TimeAbility;

impl TimeAbility {
    /// Ability name.
    pub const NAME: &'static str = "Time";

    /// The content-addressed identity of the Time ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Time RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Time ability implementation combining type info and handlers.
#[derive(Default)]
pub struct TimeRuntimeAbility;

impl TimeRuntimeAbility {
    /// Create a new Time ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for TimeRuntimeAbility {
    fn name(&self) -> &'static str {
        "Time"
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
            name: TimeAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        let now = Box::new(|_ability: &SuspendedAbility| {
            use std::time::{SystemTime, UNIX_EPOCH};
            #[allow(clippy::cast_precision_loss)]
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0);
            Ok(Value::Number(now))
        }) as HostHandler;

        let wait = Box::new(|ability: &SuspendedAbility| {
            if let Some(Value::Number(ms)) = ability.args.first() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms_u64 = if *ms < 0.0 { 0 } else { *ms as u64 };
                let duration = std::time::Duration::from_millis(ms_u64);
                std::thread::sleep(duration);
            }
            Ok(Value::Unit)
        }) as HostHandler;

        vec![(METHOD_NOW, now), (METHOD_WAIT, wait)]
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
    fn test_time_ability_constants() {
        assert_eq!(METHOD_NOW, 0x0000);
        assert_eq!(METHOD_WAIT, 0x0001);
        // Identity is stable across calls.
        assert_eq!(ability_id(), TimeAbility::ability_id());
    }

    #[test]
    fn test_time_runtime_ability_name() {
        let time = TimeRuntimeAbility::new();
        assert_eq!(time.name(), "Time");
        assert_eq!(time.ability_id(), ability_id());
    }

    #[test]
    fn test_time_descriptor_methods() {
        let time = TimeRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = time.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
        assert_eq!(descriptor.name, "Time");
        assert_eq!(descriptor.methods.len(), 2);

        // Check method names
        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"now"));
        assert!(method_names.contains(&"wait"));
    }

    #[test]
    fn test_time_now_returns_positive_number() {
        let time = TimeRuntimeAbility::new();
        let handlers = time.handlers();

        let (_, now_handler) = handlers.iter().find(|(id, _)| *id == METHOD_NOW).unwrap();

        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_NOW,
            args: vec![],
        };

        let result = now_handler(&ability);
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
        let time = TimeRuntimeAbility::new();
        let handlers = time.handlers();

        let (_, wait_handler) = handlers.iter().find(|(id, _)| *id == METHOD_WAIT).unwrap();

        // Wait for 1 millisecond
        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_WAIT,
            args: vec![Value::Number(1.0)],
        };

        let result = wait_handler(&ability);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }

    #[test]
    fn test_time_wait_handles_negative_duration() {
        let time = TimeRuntimeAbility::new();
        let handlers = time.handlers();

        let (_, wait_handler) = handlers.iter().find(|(id, _)| *id == METHOD_WAIT).unwrap();

        // Negative duration should be treated as 0
        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_WAIT,
            args: vec![Value::Number(-100.0)],
        };

        let result = wait_handler(&ability);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }
}
