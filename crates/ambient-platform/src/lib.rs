//! Platform abilities for the Ambient language.
//!
//! This crate is the native embedder layer over `ambient-engine`. It
//! ships the platform bindings interface as in-language `ability`
//! declarations ([`ABILITY_DECLARATIONS`]) and provides host handler
//! implementations that bind against the *resolved* declarations by
//! method name — there is no parallel Rust description of the interface.
//!
//! Core abilities that the language depends on (like Exception) are
//! defined in `ambient-core` instead.

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used,
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used))]

pub mod execute;
pub mod fs;
pub mod log;
pub mod network;
pub mod network_state;
pub mod process;
pub mod random;
pub mod stdio;
pub mod time;

use std::sync::Arc;

use ambient_ability::{Value, VmError};
use ambient_engine::ability_resolver::{AbilityInterface, DynAbility};
use ambient_engine::vm::Vm;

/// The platform bindings interface, as in-language `ability` declarations.
///
/// This source is the single description of the native platform: an
/// embedder parses it, resolves the declarations to content-addressed
/// identities, registers them as the `platform` ability prelude for type
/// checking and compilation, and binds host handlers against the same
/// identities by method name (see the `register_*` functions in the
/// sibling modules).
pub const ABILITY_DECLARATIONS: &str = include_str!("platform.ab");

pub use execute::{ExecuteConfig, ExecuteGrants, register_execute};
pub use fs::register_fs;
pub use log::{LogConfig, register_log};
pub use network::{NetworkConfig, register_network, register_network_shared};
pub use network_state::NetworkState;
pub use process::{
    DeployOutcome, EventSink, Functions, ProcessEvent, ProcessRuntime, ProcessRuntimeConfig,
    VmFactory, functions_from_module,
};
pub use random::register_random;
pub use stdio::{StdioConfig, StdioSink, register_stdio, register_stdio_with_collector};
pub use time::register_time;

/// Method ID for a named method of the resolved bindings interface.
///
/// Panics if the declaration is missing the method — that means the
/// bindings interface and this handler set have drifted.
pub(crate) fn require(ability: &AbilityInterface, method: &str) -> u16 {
    ability
        .method_id(method)
        .unwrap_or_else(|| panic!("platform bindings interface has no method `{method}`"))
}

/// Register the zero-config native abilities (`Stdio`, Time, Random,
/// Log, `FileSystem`) with default settings against the resolved bindings
/// interface. Network and Execute need external resources; register
/// them separately.
///
/// `Log` shares `Stdio`'s output sink, so both stream to the same stdout.
///
/// # Panics
///
/// Panics if a prelude ability with one of those names is missing a
/// method its handler set expects (the bindings interface and this
/// crate have drifted).
pub fn register_defaults(vm: &mut Vm, prelude: &[Arc<DynAbility>]) {
    let sink = StdioSink::default();
    for ability in prelude {
        let interface = AbilityInterface::from(&**ability);
        match ability.name.as_ref() {
            "Stdio" => register_stdio(vm, &interface, sink.clone(), StdioConfig::default()),
            "Time" => register_time(vm, &interface),
            "Random" => register_random(vm, &interface),
            "Log" => register_log(vm, &interface, LogConfig::default(), sink.clone()),
            "FileSystem" => register_fs(vm, &interface),
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Argument extraction helpers shared by handler implementations
// ═══════════════════════════════════════════════════════════════════════════

/// Extract a string from the first argument.
pub(crate) fn extract_string(args: &[Value]) -> Result<String, VmError> {
    match args.first() {
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(other) => Err(VmError::TypeErrorOwned {
            expected: "String".to_string(),
            got: other.type_name().to_string(),
        }),
        None => Err(VmError::TypeErrorOwned {
            expected: "String".to_string(),
            got: "no argument".to_string(),
        }),
    }
}

/// Extract a number from the first argument.
pub(crate) fn extract_number(args: &[Value]) -> Result<f64, VmError> {
    match args.first() {
        Some(Value::Number(n)) => Ok(*n),
        Some(other) => Err(VmError::TypeErrorOwned {
            expected: "Number".to_string(),
            got: other.type_name().to_string(),
        }),
        None => Err(VmError::TypeErrorOwned {
            expected: "Number".to_string(),
            got: "no argument".to_string(),
        }),
    }
}

/// Extract a `(host, port)` endpoint from the first argument.
///
/// The port is a language `number`. A value outside the `u16` range TCP
/// ports occupy — or a non-integer — is caller error, so it is raised as a
/// catchable exception rather than silently saturated by an `as` cast.
pub(crate) fn extract_host_port(args: &[Value]) -> Result<(String, u16), VmError> {
    let endpoint = match args.first() {
        Some(Value::Tuple(elements)) if elements.len() == 2 => elements,
        Some(other) => {
            return Err(VmError::TypeErrorOwned {
                expected: "(string, number) endpoint".to_string(),
                got: other.type_name().to_string(),
            });
        }
        None => {
            return Err(VmError::TypeErrorOwned {
                expected: "(string, number) endpoint".to_string(),
                got: "no argument".to_string(),
            });
        }
    };

    let host = match &endpoint[0] {
        Value::String(s) => s.to_string(),
        other => {
            return Err(VmError::TypeErrorOwned {
                expected: "string host".to_string(),
                got: other.type_name().to_string(),
            });
        }
    };

    let port = match &endpoint[1] {
        Value::Number(n) => *n,
        other => {
            return Err(VmError::TypeErrorOwned {
                expected: "number port".to_string(),
                got: other.type_name().to_string(),
            });
        }
    };

    // A wrong *type* is an engine fault (the checker should have caught it),
    // but a number outside 0..=65535 (or non-integer) is a caller mistake —
    // raise it on the catchable exception channel instead of saturating.
    if port.fract() != 0.0 || !(0.0..=f64::from(u16::MAX)).contains(&port) {
        return Err(VmError::exception(format!(
            "invalid port {port}: expected an integer in 0..=65535"
        )));
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let port = port as u16;

    Ok((host, port))
}

/// Extract bytes from a Bytes value.
pub(crate) fn extract_bytes(value: &Value) -> Result<Vec<u8>, VmError> {
    match value {
        Value::Bytes(bytes) => Ok(bytes.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "Bytes".to_string(),
            got: other.type_name().to_string(),
        }),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use ambient_engine::ability_resolver::{AbilityInterface, DynAbility, DynMethod};
    use ambient_engine::types::Type;

    /// A hand-built interface: method IDs are declaration indices, which
    /// is exactly what `resolve_ability_declarations` produces.
    pub fn test_interface(name: &str, byte: u8, methods: &[&str]) -> AbilityInterface {
        #[allow(clippy::cast_possible_truncation)]
        let methods = methods
            .iter()
            .enumerate()
            .map(|(idx, method)| DynMethod {
                id: idx as u16,
                name: Arc::from(*method),
                param_names: vec![],
                params: vec![],
                ret: Type::Unit,
                quantified: vec![],
            })
            .collect();
        let ability = DynAbility {
            id: ambient_core::AbilityId::from_bytes([byte; 32]),
            name: Arc::from(name),
            methods,
            dependencies: vec![],
        };
        AbilityInterface::from(&ability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_engine::bytecode::{BytecodeBuilder, Opcode};

    /// `register_defaults` binds handlers against whatever identities the
    /// prelude declarations resolved to, keyed by ability name.
    #[test]
    fn register_defaults_binds_by_ability_name() {
        use ambient_engine::ability_resolver::DynMethod;
        use ambient_engine::types::Type;

        let time = DynAbility {
            id: ambient_core::AbilityId::from_bytes([9; 32]),
            name: Arc::from("Time"),
            methods: vec![
                DynMethod {
                    id: 0,
                    name: Arc::from("now"),
                    param_names: vec![],
                    params: vec![],
                    ret: Type::number(),
                    quantified: vec![],
                },
                DynMethod {
                    id: 1,
                    name: Arc::from("wait"),
                    param_names: vec![Arc::from("duration")],
                    params: vec![Type::number()],
                    ret: Type::Unit,
                    quantified: vec![],
                },
            ],
            dependencies: vec![],
        };
        let prelude = vec![Arc::new(time)];

        let mut vm = Vm::new();
        register_defaults(&mut vm, &prelude);

        let mut builder = BytecodeBuilder::new();
        builder.emit_suspend(prelude[0].id, 0, 0);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);
        let func = builder.build(0, 0);
        let hash = func.hash;
        vm.load_function(func);

        let result = vm.call(&hash, vec![]);
        assert!(
            matches!(result, Ok(Value::Number(n)) if n > 0.0),
            "Time.now must dispatch to the bound handler: {result:?}"
        );
    }

    fn endpoint(host: &str, port: f64) -> Vec<Value> {
        vec![Value::tuple(vec![Value::string(host), Value::Number(port)])]
    }

    #[test]
    fn extract_host_port_accepts_valid_endpoint() {
        let got = extract_host_port(&endpoint("127.0.0.1", 8080.0)).unwrap();
        assert_eq!(got, ("127.0.0.1".to_string(), 8080));
    }

    #[test]
    fn extract_host_port_raises_exception_for_out_of_range_port() {
        // Out of the u16 range: a caller mistake, not an engine fault, so it
        // must be catchable rather than a hard VM error or a saturated cast.
        for bad in [-1.0, 65536.0, 99999.0] {
            let err = extract_host_port(&endpoint("127.0.0.1", bad)).unwrap_err();
            assert!(
                matches!(err, VmError::Exception(_)),
                "port {bad} should raise a catchable exception, got {err:?}"
            );
        }
    }

    #[test]
    fn extract_host_port_raises_exception_for_non_integer_port() {
        let err = extract_host_port(&endpoint("127.0.0.1", 80.5)).unwrap_err();
        assert!(matches!(err, VmError::Exception(_)), "got {err:?}");
    }

    #[test]
    fn extract_host_port_rejects_wrong_shape() {
        // Wrong argument types are engine faults (the checker should catch
        // them first), reported as type errors rather than exceptions.
        let not_a_tuple = vec![Value::string("127.0.0.1:8080")];
        assert!(matches!(
            extract_host_port(&not_a_tuple),
            Err(VmError::TypeErrorOwned { .. })
        ));

        let wrong_host = vec![Value::tuple(vec![Value::Number(1.0), Value::Number(80.0)])];
        assert!(matches!(
            extract_host_port(&wrong_host),
            Err(VmError::TypeErrorOwned { .. })
        ));
    }
}
