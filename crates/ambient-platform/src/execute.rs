//! Execute ability for server-side function execution.
//!
//! This ability enables a server to execute functions by their
//! content-addressed hash, supporting the remote execution protocol.
//!
//! # API
//!
//! - `has_function(hash: string) -> bool` - Check if function exists
//! - `get_dependencies(hash: string) -> List<string>` - Get function dependencies
//! - `load_functions(data: Binary) -> ()` - Load portable functions
//! - `run<T, R>(hash: string, args: T) -> R` - Execute function by hash
//! - `get_functions(hashes: List<string>) -> Binary` - Ship functions with dependencies
//! - `run_with<T, U, R>(hash: string, args: T, handler: U) -> R` - Execute with a
//!   handler value installed at the base of the isolated VM

use std::sync::{Arc, Mutex};

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;

use crate::{extract_bytes, extract_string, require};

/// Callback that grants abilities to an isolated execution VM.
pub type ExecuteGrants = Arc<dyn Fn(&mut Vm) + Send + Sync>;

/// Configuration for the Execute ability.
///
/// Execute enables server-side function execution by content-addressed hash.
pub struct ExecuteConfig {
    /// Store for function lookup.
    pub store: Arc<Mutex<Store>>,

    /// Host policy for what executed code may do: called on every fresh
    /// isolated VM to register granted host handlers (e.g. Stdio). With
    /// no grants, remote code runs pure — any perform that reaches the
    /// host is an unhandled-ability error. The remote must provide all
    /// ability handlers; nothing proxies back to the caller.
    pub grants: Option<ExecuteGrants>,
}

/// Register the Execute ability handlers on a VM.
///
/// Provides server-side function execution:
/// - `has_function(hash)` - Check if function exists
/// - `get_dependencies(hash)` - Get function dependencies
/// - `load_functions(data)` - Load functions from serialized data
/// - `run(hash, args)` - Execute function by hash
/// - `get_functions(hashes)` - Serialize functions for shipping
/// - `run_with(hash, args, handler)` - Execute with a shipped handler installed
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
#[allow(clippy::too_many_lines)]
pub fn register_execute(vm: &mut Vm, ability: &AbilityInterface, config: ExecuteConfig) {
    let store = config.store;
    let grants = config.grants;

    // Execute.has_function(hash: string) -> bool
    let store_clone = Arc::clone(&store);
    vm.register_host_handler(
        ability.id,
        require(ability, "has_function"),
        Box::new(move |ability: &SuspendedAbility| {
            let hash_str = extract_string(&ability.args)?;
            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            Ok(Value::Bool(store.contains(&hash)))
        }),
    );

    // Execute.get_dependencies(hash: string) -> List<string>
    let store_clone = Arc::clone(&store);
    vm.register_host_handler(
        ability.id,
        require(ability, "get_dependencies"),
        Box::new(move |ability: &SuspendedAbility| {
            let hash_str = extract_string(&ability.args)?;
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

    // Execute.load_functions(data: Binary) -> ()
    let store_clone = Arc::clone(&store);
    vm.register_host_handler(
        ability.id,
        require(ability, "load_functions"),
        Box::new(move |ability: &SuspendedAbility| {
            let data = match ability.args.first() {
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

    // Execute.run(hash: string, args: T) -> R
    // Executes a function by its content-addressed hash with the given argument.
    // This creates a new VM instance, loads the function and its dependencies,
    // and executes it in isolation.
    let store_clone = Arc::clone(&store);
    let grants_clone = grants.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "run"),
        Box::new(move |ability: &SuspendedAbility| {
            if ability.args.len() < 2 {
                return Err(VmError::TypeErrorOwned {
                    expected: "2 arguments (hash, args)".to_string(),
                    got: format!("{} arguments", ability.args.len()),
                });
            }

            let hash_str = match &ability.args[0] {
                Value::String(s) => s.to_string(),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "String".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };

            let arg = ability.args[1].clone();

            // Parse the hash
            let hash = parse_hash(&hash_str)
                .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

            // Get the function and all its dependencies from the store
            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;

            // Extract the function with all transitive dependencies
            let subset = store.extract_with_dependencies(&hash);
            drop(store);

            // Create a new VM for isolated execution, with whatever
            // abilities the host granted.
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
            // the executing host's own bindings (installed in `Vm::new`),
            // so an unknown uuid fails loudly at the call.
            for (native_hash, (uuid, param_count)) in subset.natives() {
                exec_vm.load_native(*native_hash, *uuid, *param_count);
            }

            // Execute the function with the provided argument
            exec_vm.call(&hash, vec![arg])
        }),
    );

    // Execute.run_with(hash: string, args: T, handler: Handler<A>) -> R
    // Like run, but installs the handler value at the base of the isolated
    // VM first, so the executed function's performs dispatch to handler
    // code that shipped with it. The handler's method functions (and their
    // dependencies) are loaded from the store alongside the target.
    let store_clone = Arc::clone(&store);
    let grants_clone2 = grants.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "run_with"),
        Box::new(move |ability: &SuspendedAbility| {
            if ability.args.len() < 3 {
                return Err(VmError::TypeErrorOwned {
                    expected: "3 arguments (hash, args, handler)".to_string(),
                    got: format!("{} arguments", ability.args.len()),
                });
            }

            let hash_str = match &ability.args[0] {
                Value::String(s) => s.to_string(),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "String".to_string(),
                        got: other.type_name().to_string(),
                    });
                }
            };
            let arg = ability.args[1].clone();
            let handler_value = match &ability.args[2] {
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

            let store = store_clone.lock().map_err(|_| VmError::LockPoisoned)?;
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

    // Execute.get_functions(hashes: List<string>) -> Binary
    let store_clone = Arc::clone(&store);
    vm.register_host_handler(
        ability.id,
        require(ability, "get_functions"),
        Box::new(move |ability: &SuspendedAbility| {
            let hashes = match ability.args.first() {
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
