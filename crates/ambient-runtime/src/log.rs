//! Log ability - for structured logging with levels.

use std::sync::OnceLock;

use ambient_ability::{format_value, HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: log a debug message.
pub const METHOD_DEBUG: u16 = 0x0000;

/// Method: log an info message.
pub const METHOD_INFO: u16 = 0x0001;

/// Method: log a warning message.
pub const METHOD_WARN: u16 = 0x0002;

/// Method: log an error message.
pub const METHOD_ERROR: u16 = 0x0003;

/// The Log ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
        MethodDescriptor {
            id: METHOD_DEBUG,
            name: "debug",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_INFO,
            name: "info",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_WARN,
            name: "warn",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_ERROR,
            name: "error",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
    ]
}

/// The content-addressed identity of the Log ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(LogAbility::NAME, &methods()))
}

/// Log ability marker.
pub const LOG: LogAbility = LogAbility;

/// Marker type for the Log ability.
#[derive(Clone, Copy)]
pub struct LogAbility;

impl LogAbility {
    /// Ability name.
    pub const NAME: &'static str = "Log";

    /// The content-addressed identity of the Log ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Log RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Log ability implementation combining type info and handlers.
///
/// Uses default stdout output.
#[derive(Default)]
pub struct LogRuntimeAbility;

impl LogRuntimeAbility {
    /// Create a new Log ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for LogRuntimeAbility {
    fn name(&self) -> &'static str {
        "Log"
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
            name: LogAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Helper macro to create log handlers
        macro_rules! make_log_handler {
            ($prefix:expr) => {
                Box::new(|ability: &SuspendedAbility| {
                    let message =
                        format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
                    #[cfg(not(test))]
                    {
                        #[allow(clippy::print_stdout)]
                        {
                            println!("[{}] {}", $prefix, message);
                        }
                    }
                    let _ = message;
                    Ok(Value::Unit)
                }) as HostHandler
            };
        }

        vec![
            (METHOD_DEBUG, make_log_handler!("DEBUG")),
            (METHOD_INFO, make_log_handler!("INFO")),
            (METHOD_WARN, make_log_handler!("WARN")),
            (METHOD_ERROR, make_log_handler!("ERROR")),
        ]
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
    fn test_log_ability_constants() {
        assert_eq!(METHOD_DEBUG, 0x0000);
        assert_eq!(METHOD_INFO, 0x0001);
        assert_eq!(METHOD_WARN, 0x0002);
        assert_eq!(METHOD_ERROR, 0x0003);
        // Identity is stable across calls.
        assert_eq!(ability_id(), LogAbility::ability_id());
    }

    #[test]
    fn test_log_runtime_ability_name() {
        let log = LogRuntimeAbility::new();
        assert_eq!(log.name(), "Log");
        assert_eq!(log.ability_id(), ability_id());
    }

    #[test]
    fn test_log_descriptor_methods() {
        let log = LogRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = log.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
        assert_eq!(descriptor.name, "Log");
        assert_eq!(descriptor.methods.len(), 4);

        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"debug"));
        assert!(method_names.contains(&"info"));
        assert!(method_names.contains(&"warn"));
        assert!(method_names.contains(&"error"));
    }

    #[test]
    fn test_log_handlers() {
        let log = LogRuntimeAbility::new();
        let handlers = log.handlers();

        assert_eq!(handlers.len(), 4);

        let method_ids: Vec<_> = handlers.iter().map(|(id, _)| *id).collect();
        assert!(method_ids.contains(&METHOD_DEBUG));
        assert!(method_ids.contains(&METHOD_INFO));
        assert!(method_ids.contains(&METHOD_WARN));
        assert!(method_ids.contains(&METHOD_ERROR));
    }

    #[test]
    fn test_log_handler_returns_unit() {
        let log = LogRuntimeAbility::new();
        let handlers = log.handlers();

        // Test debug handler
        let (_, debug_handler) = handlers.iter().find(|(id, _)| *id == METHOD_DEBUG).unwrap();

        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_DEBUG,
            args: vec![Value::string("test message")],
        };

        let result = debug_handler(&ability);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }
}
