//! Log ability - for structured logging with levels.

use ambient_ability::{format_value, HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Log ability ID.
///
/// This uses the historical ID 0x0006 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0006;

/// Method: log a debug message.
pub const METHOD_DEBUG: u16 = 0x0000;

/// Method: log an info message.
pub const METHOD_INFO: u16 = 0x0001;

/// Method: log a warning message.
pub const METHOD_WARN: u16 = 0x0002;

/// Method: log an error message.
pub const METHOD_ERROR: u16 = 0x0003;

/// Log ability marker.
pub const LOG: LogAbility = LogAbility;

/// Marker type for the Log ability.
#[derive(Clone, Copy)]
pub struct LogAbility;

impl LogAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Log";
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
        ABILITY_ID
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ABILITY_ID,
            name: "Log",
            methods: Box::leak(Box::new([
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
            ])),
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
