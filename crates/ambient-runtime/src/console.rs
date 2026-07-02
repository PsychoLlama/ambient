//! Console ability - for printing to stdout/stderr.

use std::sync::OnceLock;

use ambient_ability::{format_value, HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: print a message to stdout.
pub const METHOD_PRINT: u16 = 0x0000;

/// Method: print a message to stderr.
pub const METHOD_EPRINT: u16 = 0x0001;

/// Method: print with newline.
pub const METHOD_PRINTLN: u16 = 0x0002;

/// The Console ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
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
    ]
}

/// The content-addressed identity of the Console ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(ConsoleAbility::NAME, &methods()))
}

/// Console ability marker.
pub const CONSOLE: ConsoleAbility = ConsoleAbility;

/// Marker type for the Console ability.
#[derive(Clone, Copy)]
pub struct ConsoleAbility;

impl ConsoleAbility {
    /// Ability name.
    pub const NAME: &'static str = "Console";

    /// The content-addressed identity of the Console ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
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
        ConsoleAbility::NAME
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
            name: ConsoleAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
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
    fn test_console_ability_constants() {
        assert_eq!(METHOD_PRINT, 0x0000);
        assert_eq!(METHOD_EPRINT, 0x0001);
        assert_eq!(METHOD_PRINTLN, 0x0002);
        // Identity is stable across calls.
        assert_eq!(ability_id(), ConsoleAbility::ability_id());
    }

    #[test]
    fn test_console_runtime_ability_name() {
        let console = ConsoleRuntimeAbility::new();
        assert_eq!(console.name(), "Console");
        assert_eq!(console.ability_id(), ability_id());
    }

    #[test]
    fn test_console_descriptor_methods() {
        let console = ConsoleRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = console.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
        assert_eq!(descriptor.name, "Console");
        assert_eq!(descriptor.methods.len(), 3);

        // Check method names
        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"print"));
        assert!(method_names.contains(&"println"));
        assert!(method_names.contains(&"eprint"));
    }

    #[test]
    fn test_console_handlers() {
        let console = ConsoleRuntimeAbility::new();
        let handlers = console.handlers();

        assert_eq!(handlers.len(), 3);

        // Check handler method IDs
        let method_ids: Vec<_> = handlers.iter().map(|(id, _)| *id).collect();
        assert!(method_ids.contains(&METHOD_PRINT));
        assert!(method_ids.contains(&METHOD_PRINTLN));
        assert!(method_ids.contains(&METHOD_EPRINT));
    }

    #[test]
    fn test_console_print_handler_returns_unit() {
        let console = ConsoleRuntimeAbility::new();
        let handlers = console.handlers();

        // Find the print handler
        let (_, print_handler) = handlers.iter().find(|(id, _)| *id == METHOD_PRINT).unwrap();

        // Create a suspended ability with a string argument
        let ability = SuspendedAbility {
            ability_id: ability_id(),
            method_id: METHOD_PRINT,
            args: vec![Value::string("test message")],
        };

        let result = print_handler(&ability);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Unit);
    }
}
