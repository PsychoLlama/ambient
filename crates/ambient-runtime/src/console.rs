//! Console ability - for printing to stdout/stderr.

use ambient_ability::{format_value, HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

/// Console ability ID.
///
/// This uses the historical ID 0x0001 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0001;

/// Method: print a message to stdout.
pub const METHOD_PRINT: u16 = 0x0000;

/// Method: print a message to stderr.
pub const METHOD_EPRINT: u16 = 0x0001;

/// Method: print with newline.
pub const METHOD_PRINTLN: u16 = 0x0002;

/// Console ability marker.
pub const CONSOLE: ConsoleAbility = ConsoleAbility;

/// Marker type for the Console ability.
#[derive(Clone, Copy)]
pub struct ConsoleAbility;

impl ConsoleAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Console";
}

// ═══════════════════════════════════════════════════════════════════════════
// Console RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Console ability implementation combining type info and handlers.
///
/// Uses default stdout/stderr output.
#[derive(Default)]
pub struct ConsoleRuntimeAbility;

impl ConsoleRuntimeAbility {
    /// Create a new Console ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for ConsoleRuntimeAbility {
    fn name(&self) -> &'static str {
        "Console"
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
            name: "Console",
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: METHOD_PRINT,
                    name: "print",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_PRINTLN,
                    name: "println",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.string()],
                        return_type: |f| f.unit(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_EPRINT,
                    name: "eprint",
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
        let print = Box::new(|ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            #[cfg(not(test))]
            {
                #[allow(clippy::print_stdout)]
                {
                    println!("{message}");
                }
            }
            let _ = message;
            Ok(Value::Unit)
        }) as HostHandler;

        let println_handler = Box::new(|ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            #[cfg(not(test))]
            {
                #[allow(clippy::print_stdout)]
                {
                    println!("{message}");
                }
            }
            let _ = message;
            Ok(Value::Unit)
        }) as HostHandler;

        let eprint = Box::new(|ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            #[cfg(not(test))]
            {
                #[allow(clippy::print_stderr)]
                {
                    eprintln!("{message}");
                }
            }
            let _ = message;
            Ok(Value::Unit)
        }) as HostHandler;

        vec![
            (METHOD_PRINT, print),
            (METHOD_PRINTLN, println_handler),
            (METHOD_EPRINT, eprint),
        ]
    }
}
