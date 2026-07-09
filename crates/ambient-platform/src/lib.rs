//! Platform abilities for the Ambient language.
//!
//! This crate is the native embedder layer over `ambient-engine`. It
//! ships the platform bindings interface as in-language source
//! ([`PLATFORM_SOURCE`]): nominal `ability` declarations whose method
//! bodies — the default implementations unhandled performs run — call the
//! module's private `extern fn`s. This crate binds each extern to a
//! native implementation under a **pinned uuid** (reserved block
//! `FFFFFFFF-FFFF-FFFF-FFFC-…`, one slot per extern, forever).
//!
//! Two layers of bindings:
//!
//! - [`stub_natives`] binds every extern to a stub that raises a loud
//!   "not wired" exception. It satisfies the build-time native contract
//!   (every declaration bound, with the arity and uuid the compiler
//!   encodes) for compile-only paths. A missing capability is a
//!   configuration fault, not a recoverable operation, so the stub is a
//!   hard failure — it surfaces uncaught unless a program deliberately
//!   installs an Exception handler around the perform.
//! - The granular `*_natives` constructors ([`stdio_natives`],
//!   [`fs_natives`], ...) carry real implementations. Runtime hosts
//!   register them per VM — implementations are uuid-keyed, so later
//!   registration overwrites the stubs.
//!
//! # Fallible operations
//!
//! Operational failures on fallible platform operations (a missing file,
//! a refused connection) are **not** raised as exceptions: those methods
//! declare `Result<T, String>` return types, and their natives return an
//! in-language `Err(message)` value via [`into_result`] instead. Only
//! genuine faults — argument-type mismatches (programmer errors) and
//! unwired capabilities — still travel the [`VmError::Exception`] channel.
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
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used, clippy::unwrap_used))]

pub mod env;
pub mod execute;
pub mod fs;
pub mod network;
pub mod network_state;
pub mod process;
pub mod random;
pub mod stdio;
pub mod time;

use std::sync::Arc;

use ambient_ability::{Value, VmError};
use ambient_engine::module_path::ModulePath;
use ambient_engine::natives::{NativeFn, NativeRegistry};
use uuid::Uuid;

/// The platform bindings interface, as in-language source.
///
/// An embedder registers and compiles this as the `core::system` module
/// (`ambient_engine::build::compile_system_module`); the ability method
/// bodies inside are the default implementations unhandled performs run,
/// and their `extern fn` calls dispatch to the natives this crate binds.
pub const PLATFORM_SOURCE: &str = include_str!("platform.ab");

pub use execute::{ExecuteConfig, ExecuteGrants, execute_natives};
pub use fs::fs_natives;
pub use network::network_natives;
pub use network_state::NetworkState;
pub use process::{
    DeployOutcome, EventSink, Functions, ProcessEvent, ProcessRuntime, ProcessRuntimeConfig,
    VmFactory, functions_from_module,
};
pub use random::random_natives;
pub use stdio::{StdioConfig, StdioSink, stdio_natives, stdio_natives_with_collector};
pub use time::time_natives;

pub use env::env_natives;

// ═══════════════════════════════════════════════════════════════════════════
// The extern binding table
// ═══════════════════════════════════════════════════════════════════════════

/// One `extern fn` declaration of `platform.ab`: its compile-time name,
/// pinned uuid slot, and arity. The single source both [`stub_natives`]
/// and the real constructors bind through, so they can never drift.
#[derive(Clone, Copy)]
pub(crate) struct ExternBinding {
    pub name: &'static str,
    /// Slot within the reserved `FFFFFFFF-FFFF-FFFF-FFFC-…` block.
    pub slot: u128,
    pub arity: u8,
}

/// A pinned platform-native uuid from its block slot.
#[must_use]
pub(crate) const fn platform_uuid(slot: u128) -> Uuid {
    Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFC_0000_0000_0000 + slot)
}

/// Every extern fn in `platform.ab`, in declaration order. Slots are
/// pinned forever: never reuse or renumber one — changing an extern's
/// semantics means minting a new slot (and a new name).
pub(crate) const EXTERN_BINDINGS: &[ExternBinding] = &[
    ExternBinding {
        name: "stdio_out",
        slot: 0x01,
        arity: 1,
    },
    ExternBinding {
        name: "stdio_err",
        slot: 0x02,
        arity: 1,
    },
    ExternBinding {
        name: "stdio_read",
        slot: 0x03,
        arity: 0,
    },
    ExternBinding {
        name: "time_now",
        slot: 0x04,
        arity: 0,
    },
    ExternBinding {
        name: "time_wait",
        slot: 0x05,
        arity: 1,
    },
    ExternBinding {
        name: "random_seed",
        slot: 0x06,
        arity: 0,
    },
    ExternBinding {
        name: "random_in_range",
        slot: 0x07,
        arity: 1,
    },
    ExternBinding {
        name: "fs_read",
        slot: 0x08,
        arity: 1,
    },
    ExternBinding {
        name: "fs_write",
        slot: 0x09,
        arity: 2,
    },
    ExternBinding {
        name: "fs_read_binary",
        slot: 0x0A,
        arity: 1,
    },
    ExternBinding {
        name: "fs_write_binary",
        slot: 0x0B,
        arity: 2,
    },
    ExternBinding {
        name: "fs_exists",
        slot: 0x0C,
        arity: 1,
    },
    ExternBinding {
        name: "fs_list",
        slot: 0x0D,
        arity: 1,
    },
    ExternBinding {
        name: "fs_remove",
        slot: 0x0E,
        arity: 1,
    },
    ExternBinding {
        name: "fs_create_dir",
        slot: 0x0F,
        arity: 1,
    },
    ExternBinding {
        name: "network_listen",
        slot: 0x10,
        arity: 1,
    },
    ExternBinding {
        name: "network_accept",
        slot: 0x11,
        arity: 1,
    },
    ExternBinding {
        name: "network_close_listener",
        slot: 0x12,
        arity: 1,
    },
    ExternBinding {
        name: "network_connect",
        slot: 0x13,
        arity: 1,
    },
    ExternBinding {
        name: "network_close",
        slot: 0x14,
        arity: 1,
    },
    ExternBinding {
        name: "network_send",
        slot: 0x15,
        arity: 2,
    },
    ExternBinding {
        name: "network_receive",
        slot: 0x16,
        arity: 1,
    },
    ExternBinding {
        name: "network_local_addr",
        slot: 0x17,
        arity: 1,
    },
    ExternBinding {
        name: "network_peer_addr",
        slot: 0x18,
        arity: 1,
    },
    ExternBinding {
        name: "process_spawn",
        slot: 0x19,
        arity: 3,
    },
    ExternBinding {
        name: "process_send",
        slot: 0x1A,
        arity: 2,
    },
    ExternBinding {
        name: "process_send_named",
        slot: 0x1B,
        arity: 2,
    },
    ExternBinding {
        name: "process_self_pid",
        slot: 0x1C,
        arity: 0,
    },
    ExternBinding {
        name: "process_whereis",
        slot: 0x1D,
        arity: 1,
    },
    ExternBinding {
        name: "process_exit",
        slot: 0x1E,
        arity: 0,
    },
    ExternBinding {
        name: "env_var",
        slot: 0x1F,
        arity: 1,
    },
    ExternBinding {
        name: "env_vars",
        slot: 0x20,
        arity: 0,
    },
    ExternBinding {
        name: "env_set",
        slot: 0x21,
        arity: 2,
    },
    ExternBinding {
        name: "env_args",
        slot: 0x22,
        arity: 0,
    },
    ExternBinding {
        name: "env_cwd",
        slot: 0x23,
        arity: 0,
    },
    ExternBinding {
        name: "env_pid",
        slot: 0x24,
        arity: 0,
    },
    ExternBinding {
        name: "execute_has_function",
        slot: 0x25,
        arity: 1,
    },
    ExternBinding {
        name: "execute_get_dependencies",
        slot: 0x26,
        arity: 1,
    },
    ExternBinding {
        name: "execute_load_functions",
        slot: 0x27,
        arity: 1,
    },
    ExternBinding {
        name: "execute_run",
        slot: 0x28,
        arity: 2,
    },
    ExternBinding {
        name: "execute_get_functions",
        slot: 0x29,
        arity: 1,
    },
    ExternBinding {
        name: "execute_run_with",
        slot: 0x2A,
        arity: 3,
    },
];

/// The binding-table entry for an extern name.
///
/// # Panics
///
/// Panics if the name is not in the table — a drift between a `*_natives`
/// constructor and [`EXTERN_BINDINGS`], caught at wiring time.
pub(crate) fn binding(name: &str) -> ExternBinding {
    EXTERN_BINDINGS
        .iter()
        .copied()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("extern `{name}` is not in the platform binding table"))
}

/// The pinned uuid for an extern name (for per-VM implementation
/// registration, e.g. the process runtime's per-process natives).
///
/// # Panics
///
/// Panics if the name is not in the table.
#[must_use]
pub fn native_uuid(name: &str) -> Uuid {
    platform_uuid(binding(name).slot)
}

/// The `core::system` module path every binding attaches under.
///
/// # Panics
///
/// Never — the segments are non-empty by construction.
#[must_use]
pub(crate) fn system_module() -> ModulePath {
    #[allow(clippy::unwrap_used)]
    ModulePath::from_str_segments(&["core", "system"]).unwrap()
}

/// Register one named extern's implementation into a registry, with its
/// pinned uuid and arity from the binding table.
pub(crate) fn bind(registry: &mut NativeRegistry, name: &str, func: NativeFn) {
    let binding = binding(name);
    registry.register(
        &system_module(),
        name,
        platform_uuid(binding.slot),
        binding.arity,
        func,
    );
}

/// Every platform extern bound to a stub that raises a loud exception
/// naming the missing capability.
///
/// This is the compile-time half of the contract: builds need every
/// declared extern bound (uuid + arity) to encode native objects, whether
/// or not the running host wires the capability. Runtime hosts register
/// the real `*_natives` sets on their VMs; implementations are
/// uuid-keyed, so the real ones win.
///
/// An unwired capability is a configuration fault, not a fallible
/// operation, so the stub raises through the [`VmError::Exception`]
/// channel rather than returning an in-language `Err`: it surfaces
/// uncaught (`platform capability ... is not wired`) unless a program
/// deliberately handles it, exactly how an ungranted ability fails loudly
/// in an isolated Execute VM.
#[must_use]
pub fn stub_natives() -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    for binding in EXTERN_BINDINGS {
        let name = binding.name;
        registry.register(
            &system_module(),
            name,
            platform_uuid(binding.slot),
            binding.arity,
            Arc::new(move |_args: Vec<Value>| {
                Err(VmError::exception(format!(
                    "platform capability `{name}` is not wired in this context"
                )))
            }),
        );
    }
    registry
}

// ═══════════════════════════════════════════════════════════════════════════
// Argument extraction helpers shared by native implementations
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

/// Adapt a fallible native into an in-language `Result` value.
///
/// This is how fallible platform operations return `Result<T, String>` to
/// Ambient instead of raising a catchable exception:
///
/// - `Ok(value)` becomes `Result::Ok(value)`.
/// - `Err(VmError::Exception(message))` — an *operational* failure (missing
///   file, refused connection, invalid endpoint) — becomes
///   `Result::Err(message)`, ordinary data the caller matches on.
/// - `Err(fatal)` — a *programmer* error (argument-type or arity mismatch)
///   — stays a fatal [`VmError`], because a mistyped call is a bug the
///   checker should have caught, not a condition to recover from.
///
/// Native bodies therefore keep their natural shape (`Ok(bare_value)` on
/// success, `Err(VmError::exception(...))` on operational failure), and
/// wrapping the whole body in `into_result` is the only change needed to
/// migrate a method to a `Result` return.
pub(crate) fn into_result(outcome: Result<Value, VmError>) -> Result<Value, VmError> {
    match outcome {
        Ok(value) => Ok(Value::ok(value)),
        Err(VmError::Exception(message)) => Ok(Value::err(message)),
        Err(fatal) => Err(fatal),
    }
}

/// Extract bytes from a Binary value.
pub(crate) fn extract_bytes(value: &Value) -> Result<Vec<u8>, VmError> {
    match value {
        Value::Binary(bytes) => Ok(bytes.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "Binary".to_string(),
            got: other.type_name().to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every extern in `platform.ab` appears in the binding table with a
    /// unique, pinned slot — and vice versa. Slot assignments are content
    /// identities: changing one silently rebinds already-compiled callers.
    #[test]
    fn binding_table_matches_platform_source() {
        let declared: Vec<&str> = PLATFORM_SOURCE
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line.strip_prefix("extern fn ")?;
                let name_end = rest.find(['(', '<'])?;
                Some(&rest[..name_end])
            })
            .collect();

        let table: Vec<&str> = EXTERN_BINDINGS.iter().map(|b| b.name).collect();
        assert_eq!(
            declared, table,
            "platform.ab extern declarations and EXTERN_BINDINGS must list \
             the same names in the same order"
        );

        let mut slots: Vec<u128> = EXTERN_BINDINGS.iter().map(|b| b.slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), EXTERN_BINDINGS.len(), "slots must be unique");
    }

    /// Golden pin: the uuid block and the first/last assignments never
    /// move. (The full table is pinned structurally by the test above —
    /// slots are data, so any renumbering shows up in review; this guards
    /// the block arithmetic itself.)
    #[test]
    fn native_uuids_are_pinned() {
        assert_eq!(
            native_uuid("stdio_out").to_string(),
            "ffffffff-ffff-ffff-fffc-000000000001"
        );
        assert_eq!(
            native_uuid("execute_run_with").to_string(),
            "ffffffff-ffff-ffff-fffc-00000000002a"
        );
        assert_eq!(
            native_uuid("process_spawn").to_string(),
            "ffffffff-ffff-ffff-fffc-000000000019"
        );
    }

    #[test]
    fn stub_natives_bind_every_extern() {
        let stubs = stub_natives();
        for binding in EXTERN_BINDINGS {
            let key = stubs.key_for(&system_module(), binding.name);
            assert_eq!(
                key.map(|k| (k.uuid, k.arity)),
                Some((platform_uuid(binding.slot), binding.arity)),
                "stub for `{}`",
                binding.name
            );
        }
    }

    #[test]
    fn stubs_raise_not_wired_exceptions() {
        // An unwired capability is a hard failure on the exception channel
        // (surfaces uncaught), not an in-language `Result::Err`.
        let stubs = stub_natives();
        let func = stubs
            .impl_for(&native_uuid("network_listen"))
            .expect("stub bound");
        match func(vec![]) {
            Err(VmError::Exception(Value::String(msg))) => {
                assert!(
                    msg.contains("not wired"),
                    "stub must name the missing capability, got {msg:?}"
                );
            }
            other => panic!("expected a not-wired exception, got {other:?}"),
        }
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
