//! Content-addressed hash computation for compiled functions.
//!
//! This module implements the hash finalization phase of compilation,
//! which computes content-addressed hashes for all functions in a module.
//! Functions are identified by the hash of their bytecode content, enabling:
//! - Deduplication of identical functions
//! - Caching of compilation results
//! - Efficient equality checking
//!
//! # Hash Computation Strategy
//!
//! Functions are categorized and hashed differently:
//! 1. **Non-recursive functions**: Hash computed directly from bytecode
//! 2. **Self-recursive functions**: Hash excludes self-reference placeholder
//! 3. **Mutually recursive functions (SCC)**: Group hash computed for entire cycle
//!
//! Dependencies are resolved in topological order to ensure each function's
//! hash incorporates the final hashes of all its dependencies.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::store::compute_sccs;
use crate::value::Value;

use super::error::{CompileError, CompileErrorKind};
use super::{CompiledModule, FunctionEntry};

/// Finalize module hashes by computing content-addressed hashes.
///
/// This is the final phase of compilation that:
/// 1. Non-recursive functions: compute hash from bytecode content
/// 2. Recursive functions (SCCs): compute group hash for mutual recursion
/// 3. Lambdas: added to functions and `lambda_parents` (not `function_names`)
///
/// # Errors
///
/// Returns an error if hash computation fails due to internal inconsistencies.
#[allow(clippy::too_many_lines)]
pub(super) fn finalize_module_hashes(
    compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)>,
    lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    // Build reverse mapping: temp_hash -> name
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    // Combine named functions and lambdas for call graph analysis.
    // Lambdas use synthetic names like "__lambda_{hash}" for the call graph.
    let mut all_functions: Vec<FunctionEntry> = Vec::new();
    for (name, func, is_main) in compiled_functions {
        all_functions.push((name, func, is_main, None)); // None = not a lambda
    }
    for (temp_hash, parent_name, func) in lambdas {
        let lambda_key: Arc<str> = format!("__lambda_{temp_hash}").into();
        all_functions.push((lambda_key, func, false, Some(parent_name))); // Some = lambda with parent
    }

    // Build call graph: for each function, which other functions does it call?
    // We detect this by looking at FunctionRef values in the constant pool.
    let mut call_graph: HashMap<Arc<str>, Vec<Arc<str>>> = HashMap::new();
    for (name, func, _, _) in &all_functions {
        let mut calls = Vec::new();
        for constant in &func.constants {
            if let Value::FunctionRef(hash) = constant {
                if let Some(called_name) = temp_to_name.get(hash) {
                    calls.push(Arc::clone(called_name));
                }
            }
        }
        call_graph.insert(Arc::clone(name), calls);
    }

    // Find SCCs using Tarjan's algorithm (generic implementation from store module)
    let scc_analysis = compute_sccs(&call_graph);

    // Compute final hashes in topological order (dependencies before dependents)
    let mut final_hashes: HashMap<Arc<str>, blake3::Hash> = HashMap::new();

    // Pre-populate with imported function hashes.
    // Imported functions are already content-addressed, so we can use them directly.
    // They're in temp_hashes but not in all_functions.
    let local_names: HashSet<&Arc<str>> = all_functions.iter().map(|(n, _, _, _)| n).collect();
    for (name, hash) in temp_hashes {
        if !local_names.contains(name) {
            // This is an imported function - use its hash directly
            final_hashes.insert(Arc::clone(name), *hash);
        }
    }

    for scc in &scc_analysis.components {
        if scc.is_singleton() {
            // Single function - might be self-recursive or not
            let name = &scc.members[0];

            // Skip if this is an imported function (already in final_hashes)
            if final_hashes.contains_key(name) {
                continue;
            }

            let func = all_functions
                .iter()
                .find(|(n, _, _, _)| n == name)
                .map(|(_, f, _, _)| f)
                .ok_or_else(|| {
                    CompileError::new(
                        CompileErrorKind::Internal {
                            message: "function should exist in all_functions",
                        },
                        (0, 0),
                    )
                })?;

            // Check if it's self-recursive
            let is_self_recursive = call_graph
                .get(name)
                .is_some_and(|calls| calls.contains(name));

            if is_self_recursive {
                // Self-recursive: compute hash excluding self-reference
                let hash =
                    compute_scc_hash(&scc.members, &all_functions, &final_hashes, temp_hashes);
                final_hashes.insert(Arc::clone(name), hash);
            } else {
                // Non-recursive: compute hash with resolved dependencies
                let hash = compute_content_hash(func, &final_hashes, temp_hashes);
                final_hashes.insert(Arc::clone(name), hash);
            }
        } else {
            // Multiple functions in SCC - mutual recursion
            // Filter out imported functions (already in final_hashes)
            let local_members: Vec<&Arc<str>> = scc
                .members
                .iter()
                .filter(|name| !final_hashes.contains_key(*name))
                .collect();

            if local_members.is_empty() {
                // All members are imported - skip
                continue;
            }

            // Compute a group hash for the entire SCC
            let scc_hash =
                compute_scc_hash(&scc.members, &all_functions, &final_hashes, temp_hashes);

            // Each local function in the SCC gets a derived hash
            for (idx, name) in local_members.iter().enumerate() {
                let mut hasher = blake3::Hasher::new();
                hasher.update(scc_hash.as_bytes());
                hasher.update(&(idx as u32).to_le_bytes());
                let hash = hasher.finalize();
                final_hashes.insert(Arc::clone(name), hash);
            }
        }
    }

    // Phase 4: Update all functions with final hashes
    let mut result = CompiledModule::new();

    for (name, mut func, is_main, lambda_parent) in all_functions {
        // Update FunctionRef values in constant pool
        for constant in &mut func.constants {
            if let Value::FunctionRef(ref mut hash) = constant {
                if let Some(called_name) = temp_to_name.get(hash) {
                    if let Some(&final_hash) = final_hashes.get(called_name) {
                        *hash = final_hash;
                    }
                }
            }
        }

        // Update dependencies
        func.dependencies = func
            .dependencies
            .iter()
            .filter_map(|dep| {
                temp_to_name
                    .get(dep)
                    .and_then(|dep_name| final_hashes.get(dep_name))
                    .copied()
            })
            .collect();

        // Get the final hash for this function
        let final_hash = final_hashes.get(&name).copied().ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Internal {
                    message: "all functions should have final hashes",
                },
                (0, 0),
            )
        })?;

        // Update the function's hash field
        func.hash = final_hash;

        // Add to functions map (both named functions and lambdas)
        result.functions.insert(final_hash, func);

        if let Some(parent_name) = lambda_parent {
            // This is a lambda - add to lambda_parents, not function_names
            result.lambda_parents.insert(final_hash, parent_name);
        } else {
            // This is a named function - add to function_names
            result.function_names.insert(name, final_hash);

            if is_main {
                result.entry_point = Some(final_hash);
            }
        }
    }

    Ok(result)
}

/// Compute content-addressed hash for a non-recursive function.
fn compute_content_hash(
    func: &CompiledFunction,
    final_hashes: &HashMap<Arc<str>, blake3::Hash>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> blake3::Hash {
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    let mut hasher = blake3::Hasher::new();

    // Hash bytecode
    hasher.update(&(func.bytecode.len() as u32).to_le_bytes());
    hasher.update(&func.bytecode);

    // Hash constants with resolved function references
    hasher.update(&(func.constants.len() as u32).to_le_bytes());
    for constant in &func.constants {
        match constant {
            Value::FunctionRef(hash) => {
                // Resolve to final hash if available
                let resolved = temp_to_name
                    .get(hash)
                    .and_then(|name| final_hashes.get(name))
                    .copied()
                    .unwrap_or(*hash);
                hasher.update(&[6u8]); // TYPE_FUNCTION_REF
                hasher.update(resolved.as_bytes());
            }
            _ => hash_value_for_content(&mut hasher, constant),
        }
    }

    // Hash metadata
    hasher.update(&func.local_count.to_le_bytes());
    hasher.update(&[func.param_count]);

    // Hash resolved dependencies
    let resolved_deps: Vec<blake3::Hash> = func
        .dependencies
        .iter()
        .filter_map(|dep| {
            temp_to_name
                .get(dep)
                .and_then(|name| final_hashes.get(name))
                .copied()
        })
        .collect();

    hasher.update(&(resolved_deps.len() as u32).to_le_bytes());
    for dep in &resolved_deps {
        hasher.update(dep.as_bytes());
    }

    hasher.finalize()
}

/// Compute a combined hash for a strongly connected component (recursive functions).
fn compute_scc_hash(
    scc: &[Arc<str>],
    all_functions: &[FunctionEntry],
    final_hashes: &HashMap<Arc<str>, blake3::Hash>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> blake3::Hash {
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    // Create a set of names in this SCC for quick lookup
    let scc_set: std::collections::HashSet<&Arc<str>> = scc.iter().collect();

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"__scc__");
    hasher.update(&(scc.len() as u32).to_le_bytes());

    // Sort SCC members for deterministic ordering
    let mut sorted_scc: Vec<_> = scc.to_vec();
    sorted_scc.sort();

    for name in &sorted_scc {
        // All functions in the SCC must exist in all_functions because the SCC
        // was computed from this same set of functions.
        #[allow(clippy::expect_used)]
        let func = all_functions
            .iter()
            .find(|(n, _, _, _)| n == name)
            .map(|(_, f, _, _)| f)
            .expect("SCC function must exist in all_functions");

        // Hash the function name (for position in SCC)
        hasher.update(&(name.len() as u32).to_le_bytes());
        hasher.update(name.as_bytes());

        // Hash bytecode
        hasher.update(&(func.bytecode.len() as u32).to_le_bytes());
        hasher.update(&func.bytecode);

        // Hash constants, but use placeholders for SCC-internal references
        hasher.update(&(func.constants.len() as u32).to_le_bytes());
        for constant in &func.constants {
            match constant {
                Value::FunctionRef(hash) => {
                    if let Some(called_name) = temp_to_name.get(hash) {
                        if scc_set.contains(called_name) {
                            // Internal SCC reference - use canonical placeholder
                            hasher.update(&[6u8]); // TYPE_FUNCTION_REF
                            hasher.update(b"__scc_internal__");
                            hasher.update(called_name.as_bytes());
                        } else if let Some(&final_hash) = final_hashes.get(called_name) {
                            // External reference - use final hash
                            hasher.update(&[6u8]);
                            hasher.update(final_hash.as_bytes());
                        } else {
                            // Unknown reference - use temp hash
                            hasher.update(&[6u8]);
                            hasher.update(hash.as_bytes());
                        }
                    } else {
                        hasher.update(&[6u8]);
                        hasher.update(hash.as_bytes());
                    }
                }
                _ => hash_value_for_content(&mut hasher, constant),
            }
        }

        // Hash metadata
        hasher.update(&func.local_count.to_le_bytes());
        hasher.update(&[func.param_count]);
    }

    hasher.finalize()
}

/// Hash a value for content-addressing (mirrors bytecode.rs but accessible here).
#[allow(clippy::too_many_lines)]
fn hash_value_for_content(hasher: &mut blake3::Hasher, value: &Value) {
    const TYPE_UNIT: u8 = 0;
    const TYPE_BOOL: u8 = 1;
    const TYPE_NUMBER: u8 = 2;
    const TYPE_STRING: u8 = 3;
    const TYPE_TUPLE: u8 = 4;
    const TYPE_RECORD: u8 = 5;
    const TYPE_FUNCTION_REF: u8 = 6;
    const TYPE_SUSPENDED_ABILITY: u8 = 7;
    const TYPE_CONTINUATION: u8 = 8;

    match value {
        Value::Unit => {
            hasher.update(&[TYPE_UNIT]);
        }
        Value::Bool(b) => {
            hasher.update(&[TYPE_BOOL, u8::from(*b)]);
        }
        Value::Number(n) => {
            hasher.update(&[TYPE_NUMBER]);
            hasher.update(&n.to_bits().to_le_bytes());
        }
        Value::String(s) => {
            hasher.update(&[TYPE_STRING]);
            hasher.update(&(s.len() as u32).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        Value::Tuple(elements) => {
            hasher.update(&[TYPE_TUPLE]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Record(fields) => {
            hasher.update(&[TYPE_RECORD]);
            let mut sorted_fields: Vec<_> = fields.iter().collect();
            sorted_fields.sort_by(|a, b| a.0.cmp(b.0));
            hasher.update(&(sorted_fields.len() as u32).to_le_bytes());
            for (key, val) in sorted_fields {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
                hash_value_for_content(hasher, val);
            }
        }
        Value::FunctionRef(h) => {
            hasher.update(&[TYPE_FUNCTION_REF]);
            hasher.update(h.as_bytes());
        }
        Value::SuspendedAbility(ability) => {
            hasher.update(&[TYPE_SUSPENDED_ABILITY]);
            hasher.update(&ability.ability_id.to_le_bytes());
            hasher.update(&ability.method_id.to_le_bytes());
            hasher.update(&(ability.args.len() as u32).to_le_bytes());
            for arg in &ability.args {
                hash_value_for_content(hasher, arg);
            }
        }
        Value::Continuation(_) => {
            hasher.update(&[TYPE_CONTINUATION]);
        }
        Value::Closure(closure) => {
            const TYPE_CLOSURE: u8 = 9;
            hasher.update(&[TYPE_CLOSURE]);
            hasher.update(closure.function_hash.as_bytes());
            hasher.update(&(closure.environment.len() as u32).to_le_bytes());
            for val in &closure.environment {
                hash_value_for_content(hasher, val);
            }
        }
        Value::Handler(handler) => {
            const TYPE_HANDLER: u8 = 10;
            hasher.update(&[TYPE_HANDLER]);
            hasher.update(&handler.ability_id.to_le_bytes());
            // Hash methods in sorted order for deterministic hashing
            let mut methods: Vec<_> = handler.methods.iter().collect();
            methods.sort_by_key(|(k, _)| *k);
            hasher.update(&(methods.len() as u32).to_le_bytes());
            for (method_id, func_hash) in methods {
                hasher.update(&method_id.to_le_bytes());
                hasher.update(func_hash.as_bytes());
            }
            // Hash captures
            hasher.update(&(handler.captures.len() as u32).to_le_bytes());
            for val in &handler.captures {
                hash_value_for_content(hasher, val);
            }
        }
        Value::List(elements) => {
            const TYPE_LIST: u8 = 11;
            hasher.update(&[TYPE_LIST]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Map(map) => {
            const TYPE_MAP: u8 = 12;
            hasher.update(&[TYPE_MAP]);
            // BTreeMap is already sorted, so iteration order is deterministic
            hasher.update(&(map.entries.len() as u32).to_le_bytes());
            for (key, val) in &map.entries {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
                hash_value_for_content(hasher, val);
            }
        }
        Value::Set(set) => {
            const TYPE_SET: u8 = 13;
            hasher.update(&[TYPE_SET]);
            hasher.update(&(set.elements.len() as u32).to_le_bytes());
            for elem in &set.elements {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Enum(e) => {
            const TYPE_ENUM: u8 = 14;
            hasher.update(&[TYPE_ENUM]);
            // Hash type name
            hasher.update(&(e.type_name.len() as u32).to_le_bytes());
            hasher.update(e.type_name.as_bytes());
            // Hash tag
            hasher.update(&e.tag.to_le_bytes());
            // Hash variant name
            hasher.update(&(e.variant_name.len() as u32).to_le_bytes());
            hasher.update(e.variant_name.as_bytes());
            // Hash payload (if any)
            if let Some(payload) = e.payload.as_deref() {
                hasher.update(&[1u8]); // has payload marker
                hash_value_for_content(hasher, payload);
            } else {
                hasher.update(&[0u8]); // no payload marker
            }
        }
        Value::Module(m) => {
            const TYPE_MODULE: u8 = 15;
            hasher.update(&[TYPE_MODULE]);
            hasher.update(&(m.path.len() as u32).to_le_bytes());
            hasher.update(m.path.as_bytes());
        }
        Value::ModuleMember(m) => {
            const TYPE_MODULE_MEMBER: u8 = 16;
            hasher.update(&[TYPE_MODULE_MEMBER]);
            hasher.update(&(m.path.len() as u32).to_le_bytes());
            hasher.update(m.path.as_bytes());
        }
    }
}

/// Compute a temporary hash for a function based on its name.
///
/// This is only used during the initial compilation pass; the final
/// content-addressed hash is computed after all functions are compiled.
pub(super) fn compute_temporary_hash(name: &str) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"__temp_hash__");
    hasher.update(name.as_bytes());
    hasher.finalize()
}
