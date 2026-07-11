//! Platform abilities for the Ambient language.
//!
//! This crate is the native embedder layer over `ambient-engine`. It
//! ships the platform bindings interface as an in-language source tree
//! ([`platform_modules`]): the `core::system` directory module and one
//! submodule per ability, nominal `ability` declarations whose method
//! bodies — the default implementations unhandled performs run — call each
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
//!   installs an Exception handler around the perform. (One documented
//!   deviation: `live_latest` stubs to identity — see [`stub_natives`].)
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

pub mod deploy;
pub mod env;
pub mod execute;
pub mod fs;
pub mod network;
pub mod network_state;
pub mod process;
pub mod random;
pub mod stdio;
pub mod time;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use ambient_ability::{Value, VmError};
use ambient_engine::core_library::DeclModule;
use ambient_engine::module_path::ModulePath;
use ambient_engine::natives::{NativeFn, NativeRegistry};
use include_dir::{Dir, DirEntry, include_dir};
use uuid::Uuid;

/// The platform bindings interface, as an in-language source tree.
///
/// `core::system` is a directory module: its shape is this `platform/` tree
/// — a `main.ab` re-exporting one submodule per ability (`stdio`, `time`,
/// `fs`, ...). An embedder registers and compiles it with
/// [`ambient_engine::build::compile_declaration_modules`]; the ability
/// method bodies inside are the default implementations unhandled performs
/// run, and their `extern fn` calls dispatch to the natives this crate binds.
static PLATFORM_LIB: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/src/platform");

/// Every module of the platform bindings interface, each under its reserved
/// `core::system::*` path, in a deterministic order (by module path). Pass
/// this to [`ambient_engine::build::compile_declaration_modules`] or
/// [`ambient_engine::core_library::register_declaration_modules`].
#[must_use]
pub fn platform_modules() -> &'static [DeclModule<'static>] {
    static MODULES: OnceLock<Vec<DeclModule<'static>>> = OnceLock::new();
    MODULES.get_or_init(|| {
        let mut modules = Vec::new();
        collect_platform_modules(&PLATFORM_LIB, &mut modules);
        modules.sort_by_key(|module| module.path.to_string());
        modules
    })
}

/// Recursively gather the `.ab` files under `dir` into `out`, mapping each
/// to its `core::system::*` module path.
fn collect_platform_modules(dir: &'static Dir<'static>, out: &mut Vec<DeclModule<'static>>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => collect_platform_modules(child, out),
            DirEntry::File(file) => {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("ab") {
                    continue;
                }
                let (Some((module_path, is_dir_module)), Some(source)) =
                    (platform_module_path(path), file.contents_utf8())
                else {
                    continue;
                };
                out.push(DeclModule {
                    path: module_path,
                    source,
                    is_dir_module,
                });
            }
        }
    }
}

/// The `core::system::*` module path (and directory-module flag) for a file
/// path relative to `platform/`, derived through the canonical file↔module
/// mapping so it never forks from how core and user packages map files.
///
/// The tree's own root `main.ab` is the `core::system` module itself — a
/// directory module whose members are the per-ability submodules.
fn platform_module_path(relative: &Path) -> Option<(ModulePath, bool)> {
    let (relative_path, is_dir_module) = ModulePath::from_relative_file_path_with_kind(relative)?;
    if relative_path == ModulePath::root() {
        return Some((ModulePath::from_str_segments(&["core", "system"])?, true));
    }
    let mut segments: Vec<Arc<str>> = vec![Arc::from("core"), Arc::from("system")];
    segments.extend(relative_path.segments().iter().cloned());
    Some((ModulePath::from_segments(segments)?, is_dir_module))
}

pub use deploy::{
    Binding, DeployError, DeployReport, DeployRuntime, Functions, Generation, NameDiff, NameTable,
    VmFactory, functions_from_module,
};
pub use execute::{ExecuteConfig, ExecuteGrants, execute_natives};
pub use fs::fs_natives;
pub use network::network_natives;
pub use network_state::NetworkState;
pub use process::{DeployOutcome, EventSink, ProcessEvent, ProcessRuntime, ProcessRuntimeConfig};
pub use random::random_natives;
pub use stdio::{StdioConfig, StdioSink, stdio_natives, stdio_natives_with_collector};
pub use time::time_natives;

pub use env::env_natives;

// ═══════════════════════════════════════════════════════════════════════════
// The extern binding table
// ═══════════════════════════════════════════════════════════════════════════

/// One `extern fn` declaration of the platform interface: its compile-time
/// name, the ability submodule it is declared in (`core::system::<module>`,
/// the [`NativeRegistry`] key), its pinned uuid slot, and arity. The single
/// source both [`stub_natives`] and the real constructors bind through, so
/// they can never drift.
#[derive(Clone, Copy)]
pub(crate) struct ExternBinding {
    pub name: &'static str,
    /// The leaf of the declaring submodule path (`core::system::<module>`).
    /// Each ability's `extern fn`s live in its own file now, so natives must
    /// key under that submodule — not a flat `core::system`.
    pub module: &'static str,
    /// Slot within the reserved `FFFFFFFF-FFFF-FFFF-FFFC-…` block.
    pub slot: u128,
    pub arity: u8,
}

/// A pinned platform-native uuid from its block slot.
#[must_use]
pub(crate) const fn platform_uuid(slot: u128) -> Uuid {
    Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFC_0000_0000_0000 + slot)
}

/// Every extern fn across the platform tree, keyed to its declaring
/// submodule. Slots are pinned forever: never reuse or renumber one —
/// changing an extern's semantics means minting a new slot (and a new name).
pub(crate) const EXTERN_BINDINGS: &[ExternBinding] = &[
    ExternBinding {
        name: "stdio_out",
        module: "stdio",
        slot: 0x01,
        arity: 1,
    },
    ExternBinding {
        name: "stdio_err",
        module: "stdio",
        slot: 0x02,
        arity: 1,
    },
    ExternBinding {
        name: "stdio_read",
        module: "stdio",
        slot: 0x03,
        arity: 0,
    },
    ExternBinding {
        name: "time_now",
        module: "time",
        slot: 0x04,
        arity: 0,
    },
    ExternBinding {
        name: "time_wait",
        module: "time",
        slot: 0x05,
        arity: 1,
    },
    ExternBinding {
        name: "random_seed",
        module: "random",
        slot: 0x06,
        arity: 0,
    },
    ExternBinding {
        name: "random_in_range",
        module: "random",
        slot: 0x07,
        arity: 1,
    },
    ExternBinding {
        name: "fs_read",
        module: "fs",
        slot: 0x08,
        arity: 1,
    },
    ExternBinding {
        name: "fs_write",
        module: "fs",
        slot: 0x09,
        arity: 2,
    },
    ExternBinding {
        name: "fs_read_binary",
        module: "fs",
        slot: 0x0A,
        arity: 1,
    },
    ExternBinding {
        name: "fs_write_binary",
        module: "fs",
        slot: 0x0B,
        arity: 2,
    },
    ExternBinding {
        name: "fs_exists",
        module: "fs",
        slot: 0x0C,
        arity: 1,
    },
    ExternBinding {
        name: "fs_list",
        module: "fs",
        slot: 0x0D,
        arity: 1,
    },
    ExternBinding {
        name: "fs_remove",
        module: "fs",
        slot: 0x0E,
        arity: 1,
    },
    ExternBinding {
        name: "fs_create_dir",
        module: "fs",
        slot: 0x0F,
        arity: 1,
    },
    ExternBinding {
        name: "network_listen",
        module: "network",
        slot: 0x10,
        arity: 1,
    },
    ExternBinding {
        name: "network_accept",
        module: "network",
        slot: 0x11,
        arity: 1,
    },
    ExternBinding {
        name: "network_close_listener",
        module: "network",
        slot: 0x12,
        arity: 1,
    },
    ExternBinding {
        name: "network_connect",
        module: "network",
        slot: 0x13,
        arity: 1,
    },
    ExternBinding {
        name: "network_close",
        module: "network",
        slot: 0x14,
        arity: 1,
    },
    ExternBinding {
        name: "network_send",
        module: "network",
        slot: 0x15,
        arity: 2,
    },
    ExternBinding {
        name: "network_receive",
        module: "network",
        slot: 0x16,
        arity: 1,
    },
    ExternBinding {
        name: "network_local_addr",
        module: "network",
        slot: 0x17,
        arity: 1,
    },
    ExternBinding {
        name: "network_peer_addr",
        module: "network",
        slot: 0x18,
        arity: 1,
    },
    ExternBinding {
        name: "process_spawn",
        module: "process",
        slot: 0x19,
        arity: 3,
    },
    ExternBinding {
        name: "process_send",
        module: "process",
        slot: 0x1A,
        arity: 2,
    },
    ExternBinding {
        name: "process_send_named",
        module: "process",
        slot: 0x1B,
        arity: 2,
    },
    ExternBinding {
        name: "process_self_pid",
        module: "process",
        slot: 0x1C,
        arity: 0,
    },
    ExternBinding {
        name: "process_whereis",
        module: "process",
        slot: 0x1D,
        arity: 1,
    },
    ExternBinding {
        name: "process_exit",
        module: "process",
        slot: 0x1E,
        arity: 0,
    },
    ExternBinding {
        name: "env_var",
        module: "env",
        slot: 0x1F,
        arity: 1,
    },
    ExternBinding {
        name: "env_vars",
        module: "env",
        slot: 0x20,
        arity: 0,
    },
    ExternBinding {
        name: "env_set",
        module: "env",
        slot: 0x21,
        arity: 2,
    },
    ExternBinding {
        name: "env_args",
        module: "env",
        slot: 0x22,
        arity: 0,
    },
    ExternBinding {
        name: "env_cwd",
        module: "env",
        slot: 0x23,
        arity: 0,
    },
    ExternBinding {
        name: "env_pid",
        module: "env",
        slot: 0x24,
        arity: 0,
    },
    ExternBinding {
        name: "execute_has_function",
        module: "execute",
        slot: 0x25,
        arity: 1,
    },
    ExternBinding {
        name: "execute_get_dependencies",
        module: "execute",
        slot: 0x26,
        arity: 1,
    },
    ExternBinding {
        name: "execute_load_functions",
        module: "execute",
        slot: 0x27,
        arity: 1,
    },
    ExternBinding {
        name: "execute_run",
        module: "execute",
        slot: 0x28,
        arity: 2,
    },
    ExternBinding {
        name: "execute_get_functions",
        module: "execute",
        slot: 0x29,
        arity: 1,
    },
    ExternBinding {
        name: "execute_run_with",
        module: "execute",
        slot: 0x2A,
        arity: 3,
    },
    ExternBinding {
        name: "live_latest",
        module: "live",
        slot: 0x2B,
        arity: 1,
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

/// The `core::system::<module>` path an extern's native attaches under —
/// the ability submodule its `extern fn` is declared in. `ModuleEnv::new`
/// looks natives up by the declaring module's path, so a stdio native must
/// key under `core::system::stdio`, not a flat `core::system`.
///
/// # Panics
///
/// Never — the segments are non-empty by construction.
#[must_use]
pub(crate) fn extern_module(binding: ExternBinding) -> ModulePath {
    #[allow(clippy::unwrap_used)]
    ModulePath::from_str_segments(&["core", "system", binding.module]).unwrap()
}

/// Register one named extern's implementation into a registry, with its
/// pinned uuid and arity from the binding table, under its declaring
/// submodule path.
pub(crate) fn bind(registry: &mut NativeRegistry, name: &str, func: NativeFn) {
    let binding = binding(name);
    registry.register(
        &extern_module(binding),
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
///
/// One deliberate exception: `live_latest` stubs to *identity*, because
/// `Live::latest!`'s contract is "the current binding, or `f` itself when
/// no deploy runtime is present" — a VM without a deploy runtime is the
/// no-runtime case, not a misconfiguration (see `ref/live-upgrade.md`).
#[must_use]
pub fn stub_natives() -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    for binding in EXTERN_BINDINGS {
        let name = binding.name;
        let func: NativeFn = if name == "live_latest" {
            Arc::new(deploy::latest_identity)
        } else {
            Arc::new(move |_args: Vec<Value>| {
                Err(VmError::exception(format!(
                    "platform capability `{name}` is not wired in this context"
                )))
            })
        };
        registry.register(
            &extern_module(*binding),
            name,
            platform_uuid(binding.slot),
            binding.arity,
            func,
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

    /// Every `extern fn` across the platform tree appears in the binding
    /// table under its declaring submodule with a unique, pinned slot — and
    /// vice versa. Slot assignments are content identities: changing one
    /// silently rebinds already-compiled callers. The submodule must match
    /// too: `ModuleEnv` keys natives by declaring module, so a wrong module
    /// leaves an extern unbound at compile time.
    #[test]
    fn binding_table_matches_platform_source() {
        use std::collections::BTreeMap;

        // extern name -> declaring submodule leaf, gathered from the tree.
        let mut declared: BTreeMap<&str, String> = BTreeMap::new();
        for module in platform_modules() {
            let leaf = module
                .path
                .segments()
                .last()
                .map(ToString::to_string)
                .unwrap_or_default();
            for line in module.source.lines() {
                let line = line.trim();
                let Some(rest) = line.strip_prefix("extern fn ") else {
                    continue;
                };
                let Some(name_end) = rest.find(['(', '<']) else {
                    continue;
                };
                let name = &rest[..name_end];
                assert!(
                    declared.insert(name, leaf.clone()).is_none(),
                    "duplicate extern `{name}`"
                );
            }
        }

        let table: BTreeMap<&str, String> = EXTERN_BINDINGS
            .iter()
            .map(|b| (b.name, b.module.to_string()))
            .collect();
        assert_eq!(
            declared, table,
            "the platform tree's `extern fn` declarations and EXTERN_BINDINGS \
             must agree on the same names, each under its declaring submodule"
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
        assert_eq!(
            native_uuid("live_latest").to_string(),
            "ffffffff-ffff-ffff-fffc-00000000002b"
        );
    }

    #[test]
    fn stub_natives_bind_every_extern() {
        let stubs = stub_natives();
        for binding in EXTERN_BINDINGS {
            let key = stubs.key_for(&extern_module(*binding), binding.name);
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

    /// The one stub that must never raise: `Live::latest!` under plain
    /// `ambient run` (no deploy runtime) is identity by design — but it
    /// still enforces the function-shaped contract the real resolution
    /// enforces, so `run` and `dev` reject the same programs.
    #[test]
    fn live_latest_stub_is_identity() {
        let stubs = stub_natives();
        let func = stubs
            .impl_for(&native_uuid("live_latest"))
            .expect("stub bound");

        let hash = blake3::hash(b"some function");
        match func(vec![Value::FunctionRef(hash)]) {
            Ok(Value::FunctionRef(returned)) => assert_eq!(returned, hash),
            other => panic!("expected the ref back unchanged, got {other:?}"),
        }

        match func(vec![Value::Number(3.0)]) {
            Err(VmError::Exception(Value::String(msg))) => {
                assert!(msg.contains("expected a function"), "got {msg:?}");
            }
            other => panic!("expected a function-contract exception, got {other:?}"),
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
