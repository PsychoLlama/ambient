//! Execute natives for server-side function execution.
//!
//! Enables a server to execute functions by their content-addressed hash,
//! supporting the remote execution protocol. Executed code runs in an
//! **isolated VM**: a fresh `Vm::new()` carries only the engine's core
//! natives (pure value transformations), so the platform capabilities an
//! executed function can reach are exactly what the host's grants closure
//! registers — that closure is the capability boundary.

use std::sync::{Arc, Mutex};

use ambient_ability::{Value, VmError};
use ambient_engine::natives::NativeRegistry;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;

use crate::{bind, extract_bytes, extract_string};

/// Callback that grants abilities to an isolated execution VM.
///
/// Called on every fresh isolated VM before executed code runs. Under the
/// nominal-ability model an unhandled perform runs the ability's default
/// implementation, whose extern calls dispatch against the VM's native
/// table — so granting a capability means registering its natives
/// (`exec_vm.register_natives(&stdio_natives(...))`). An ungranted
/// capability fails loudly at the call (`UnboundNative`).
pub type ExecuteGrants = Arc<dyn Fn(&mut Vm) + Send + Sync>;

/// Configuration for the Execute natives.
///
/// Execute enables server-side function execution by content-addressed hash.
pub struct ExecuteConfig {
    /// Store for function lookup.
    pub store: Arc<Mutex<Store>>,

    /// Host policy for what executed code may do: called on every fresh
    /// isolated VM to register granted natives (e.g. Stdio's). With no
    /// grants, remote code runs pure — any platform extern it reaches is
    /// an unbound-native error. The remote must provide all capabilities;
    /// nothing proxies back to the caller.
    pub grants: Option<ExecuteGrants>,
}

/// The `Execute` native implementations.
///
/// Provides server-side function execution:
/// - `execute_has_function(hash)` - Check if function exists
/// - `execute_get_dependencies(hash)` - Get function dependencies
/// - `execute_load_functions(data)` - Load functions from serialized data
/// - `execute_run(hash, args)` - Execute function by hash
/// - `execute_get_functions(hashes)` - Serialize functions for shipping
/// - `execute_run_with(hash, args, handler)` - Execute with a shipped
///   handler installed
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn execute_natives(config: ExecuteConfig) -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    let store = config.store;
    let grants = config.grants;

    // execute_has_function(hash: string) -> bool
    let store_clone = Arc::clone(&store);
    bind(
        &mut registry,
        "execute_has_function",
        Arc::new(move |args: Vec<Value>| {
            let hash_str = extract_string(&args)?;
            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            Ok(Value::Bool(store.contains(&hash)))
        }),
    );

    // execute_get_dependencies(hash: string) -> List<string>
    let store_clone = Arc::clone(&store);
    bind(
        &mut registry,
        "execute_get_dependencies",
        Arc::new(move |args: Vec<Value>| {
            let hash_str = extract_string(&args)?;
            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            let deps = store.missing_dependencies(&hash);
            let dep_strings: Vec<Value> = deps
                .iter()
                .map(|h| Value::string(h.to_hex().to_string()))
                .collect();
            Ok(Value::list(dep_strings))
        }),
    );

    // execute_load_functions(data: Binary) -> ()
    let store_clone = Arc::clone(&store);
    bind(
        &mut registry,
        "execute_load_functions",
        Arc::new(move |args: Vec<Value>| {
            let data = match args.first() {
                Some(v) => extract_bytes(v)?,
                None => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "Binary".to_string(),
                        got: "no argument".to_string(),
                    });
                }
            };

            // Decode the canonical object pack. Hashes are recomputed from
            // the object bytes, never trusted from the sender.
            let mut store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            store
                .add_pack(&data)
                .map_err(|e| VmError::IoError(format!("invalid function pack: {e}")))?;
            Ok(Value::Unit)
        }),
    );

    // execute_run(hash: string, args: T) -> R
    // Executes a function by its content-addressed hash with the given
    // argument, in a fresh isolated VM.
    let store_clone = Arc::clone(&store);
    let grants_clone = grants.clone();
    bind(
        &mut registry,
        "execute_run",
        Arc::new(move |args: Vec<Value>| {
            if args.len() < 2 {
                return Err(VmError::TypeErrorOwned {
                    expected: "2 arguments (hash, args)".to_string(),
                    got: format!("{} arguments", args.len()),
                });
            }

            let hash_str = match &args[0] {
                Value::String(s) => s.to_string(),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "String".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };

            let arg = args[1].clone();

            // Parse the hash
            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            // Get the function and all its dependencies from the store
            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;

            // Extract the function with all transitive dependencies
            let subset = store.extract_with_dependencies(&hash);
            drop(store);

            // Create a new VM for isolated execution, with whatever
            // capabilities the host granted.
            let mut exec_vm = Vm::new();
            if let Some(grants) = &grants_clone {
                grants(&mut exec_vm);
            }

            // Load all functions (the target and its dependencies)
            for func_hash in subset.hashes() {
                if let Some(func) = subset.get(&func_hash) {
                    exec_vm.load_function(func.as_ref().clone());
                }
            }
            // Load the `const` value objects those functions depend on.
            for (value_hash, value) in subset.values() {
                exec_vm.load_value(*value_hash, value.clone());
            }
            // Load native (extern fn) objects; implementations come from
            // the executing host's own bindings (core natives from
            // `Vm::new`, platform natives from the grants closure), so an
            // unknown uuid fails loudly at the call.
            for (native_hash, (uuid, param_count)) in subset.natives() {
                exec_vm.load_native(*native_hash, *uuid, *param_count);
            }

            // Execute the function with the provided argument
            exec_vm.call(&hash, vec![arg])
        }),
    );

    // execute_get_functions(hashes: List<string>) -> Binary
    let store_clone = Arc::clone(&store);
    bind(
        &mut registry,
        "execute_get_functions",
        Arc::new(move |args: Vec<Value>| {
            let hashes = match args.first() {
                Some(Value::List(list)) => list.clone(),
                Some(other) => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "list".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
                None => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "list".to_string(),
                        got: "no argument".to_string(),
                    });
                }
            };

            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;

            // Collect every requested function plus transitive dependencies
            // into one deduplicated store, then ship its canonical objects
            // as a pack.
            let mut subset = Store::new();
            for hash_value in hashes.iter() {
                let hash_str = match hash_value {
                    Value::String(s) => s,
                    other => {
                        return Err(VmError::TypeErrorOwned {
                            expected: "String".to_string(),
                            got: other.type_name().to_string(),
                        });
                    }
                };

                let hash = parse_hash(hash_str)
                    .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

                subset.merge(&store.extract_with_dependencies(&hash));
            }
            drop(store);

            let bytes = subset
                .serialize()
                .map_err(|e| VmError::IoError(format!("serialize error: {e}")))?;

            Ok(Value::binary(bytes))
        }),
    );

    // execute_run_with(hash: string, args: T, handler: Handler<A>) -> R
    // Like execute_run, but installs the handler value at the base of the
    // isolated VM first, so the executed function's performs dispatch to
    // handler code that shipped with it. The handler's method functions
    // (and their dependencies) are loaded from the store alongside the
    // target.
    let grants_clone2 = grants.clone();
    bind(
        &mut registry,
        "execute_run_with",
        Arc::new(move |args: Vec<Value>| {
            if args.len() < 3 {
                return Err(VmError::TypeErrorOwned {
                    expected: "3 arguments (hash, args, handler)".to_string(),
                    got: format!("{} arguments", args.len()),
                });
            }

            let hash_str = match &args[0] {
                Value::String(s) => s.to_string(),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "String".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };
            let arg = args[1].clone();
            let handler_value = match &args[2] {
                Value::Handler(h) => Arc::clone(h),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "handler".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };

            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            let store = store.lock().map_err(|_| VmError::LockPoisoned)?;
            let mut subset = store.extract_with_dependencies(&hash);
            for method_hash in handler_value.methods.values() {
                subset.merge(&store.extract_with_dependencies(method_hash));
            }
            drop(store);

            let mut exec_vm = Vm::new();
            if let Some(grants) = &grants_clone2 {
                grants(&mut exec_vm);
            }
            for func_hash in subset.hashes() {
                if let Some(func) = subset.get(&func_hash) {
                    exec_vm.load_function(func.as_ref().clone());
                }
            }
            for (value_hash, value) in subset.values() {
                exec_vm.load_value(*value_hash, value.clone());
            }
            for (native_hash, (uuid, param_count)) in subset.natives() {
                exec_vm.load_native(*native_hash, *uuid, *param_count);
            }

            exec_vm.install_base_handler(handler_value);
            exec_vm.call(&hash, vec![arg])
        }),
    );

    registry
}

/// Parse a hex-encoded hash string.
fn parse_hash(hex_str: &str) -> Result<blake3::Hash, String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "invalid hash length: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(blake3::Hash::from_bytes(arr))
}
