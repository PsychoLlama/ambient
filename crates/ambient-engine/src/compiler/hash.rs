//! Content-addressed hash finalization.
//!
//! This is the final phase of compilation. Functions are compiled against
//! temporary name-derived hashes; this phase groups them into canonical
//! objects (see [`crate::object`]) and derives every function's final hash
//! from its object's encoding:
//!
//! - Non-recursive functions become **plain objects**:
//!   `hash = blake3(encoding)`.
//! - Each strongly connected component of recursive functions becomes one
//!   **group object**. Intra-group references are encoded as member indices,
//!   breaking the circularity; member hashes are derived from the group hash.
//!
//! Because a hash is literally the blake3 of the object's bytes, any object
//! can be verified anywhere (disk, wire, another machine) without trusting
//! the sender: re-hash the bytes and compare.
//!
//! # Determinism
//!
//! Group members are ordered canonically (named members sorted by name, then
//! lambdas in first-reference order), so hashes do not depend on declaration
//! order or on compilation-internal counters. Names of recursive functions
//! are part of their group's identity — members of a cycle are only
//! distinguishable by name — so renaming a recursive function changes its
//! hash. Renaming a non-recursive function never does.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::fqn::NameKey;
use crate::object::{GroupMember, ObjectRef, StoredObject, function_from_compiled, member_hash};
use crate::store::compute_sccs_with_cmp;
use crate::value::Value;

use super::CompiledModule;
use super::error::{CompileError, CompileErrorKind};

/// Content-address every module-level `const`, in a pre-pass before function
/// bodies compile.
///
/// A `const` value object is `blake3(encode(value))` — a pure function of the
/// value's type and bytes, independent of the const's name — so two consts
/// with the same value produce byte-identical objects and one shared hash.
/// Because a `const` literal references nothing (the literal-only rule), each
/// object is a self-contained leaf and needs no call-graph pass: its hash is
/// known immediately, so function bodies compiled afterward can link to it.
///
/// Returns the `NameKey → final hash` table (seeded into the compiler so a
/// reference emits `LoadObject`) and the `hash → value object` map (folded
/// into the module's objects so the value ships and deduplicates like any
/// other content-addressed object). Values that cannot be content-addressed
/// are skipped; a reference to one then surfaces as an undefined-name error,
/// exactly as a non-literal `const` does today.
#[must_use]
pub(super) fn finalize_const_values(
    consts: &[(NameKey, Value)],
) -> (
    HashMap<NameKey, blake3::Hash>,
    HashMap<blake3::Hash, StoredObject>,
) {
    let mut hashes = HashMap::new();
    let mut objects = HashMap::new();
    for (key, value) in consts {
        if let Ok(object) = crate::object::value_object(value) {
            let hash = object.hash();
            hashes.insert(key.clone(), hash);
            objects.insert(hash, object);
        }
    }
    (hashes, objects)
}

/// A function awaiting finalization.
struct Node {
    /// Temporary (pre-finalization) hash: name-derived for named functions,
    /// counter-derived for lambdas.
    temp: blake3::Hash,
    /// Source name; `None` for lambdas.
    name: Option<Arc<str>>,
    func: CompiledFunction,
    is_main: bool,
    lambda_parent: Option<Arc<str>>,
}

fn internal_error(message: &'static str) -> CompileError {
    CompileError::new(CompileErrorKind::Internal { message }, (0, 0))
}

/// Compute final content-addressed hashes for all functions in a module.
///
/// # Errors
///
/// Returns an error if a constant pool contains values that cannot be
/// content-addressed, or on internal inconsistencies.
pub(super) fn finalize_module_hashes(
    compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)>,
    lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
) -> Result<CompiledModule, CompileError> {
    // References to imported functions already carry final hashes and pass
    // through unchanged; only references to local temp hashes get rewritten.
    let mut nodes: Vec<Node> = Vec::new();
    for (name, func, is_main) in compiled_functions {
        nodes.push(Node {
            temp: func.hash,
            name: Some(name),
            func,
            is_main,
            lambda_parent: None,
        });
    }
    for (temp, parent, func) in lambdas {
        nodes.push(Node {
            temp,
            name: None,
            func,
            is_main: false,
            lambda_parent: Some(parent),
        });
    }

    // Temp hash -> node index, for classifying references as local.
    let local: HashMap<blake3::Hash, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.temp, i)).collect();

    // Call graph over node indices, from constant-pool function references.
    let mut graph: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let mut edges = Vec::new();
        for constant in &node.func.constants {
            if let Value::FunctionRef(h) = constant
                && let Some(&j) = local.get(h)
            {
                edges.push(j);
            }
        }
        graph.insert(i, edges);
    }

    // SCCs in reverse topological order: dependencies before dependents.
    let sccs = compute_sccs_with_cmp(&graph, std::cmp::Ord::cmp);

    let mut final_hashes: HashMap<usize, blake3::Hash> = HashMap::new();
    let mut objects: HashMap<blake3::Hash, StoredObject> = HashMap::new();

    for scc in &sccs.components {
        let members: HashSet<usize> = scc.members.iter().copied().collect();

        let is_cycle = scc.members.len() > 1 || {
            let i = scc.members[0];
            graph[&i].contains(&i)
        };

        if is_cycle {
            finalize_group(
                &scc.members,
                &members,
                &nodes,
                &local,
                &mut final_hashes,
                &mut objects,
            )?;
        } else {
            let i = scc.members[0];
            let subst = build_substitution(&[i], &members, &nodes, &local, &final_hashes, &|_| {
                // Unreachable: a non-cycle singleton has no internal refs.
                ObjectRef::External(nodes[i].temp)
            })?;
            let object = StoredObject::Plain(
                function_from_compiled(&nodes[i].func, &|h| resolve_ref(&subst, h)).map_err(
                    |_| internal_error("constant pool value cannot be content-addressed"),
                )?,
            );
            let hash = object.hash();
            final_hashes.insert(i, hash);
            objects.insert(hash, object);
        }
    }

    // Build the final module: substitute final hashes into every function.
    let mut result = CompiledModule::new();
    result.objects = objects;

    for (i, node) in nodes.iter().enumerate() {
        let final_hash = *final_hashes
            .get(&i)
            .ok_or_else(|| internal_error("all functions should have final hashes"))?;

        let mut func = node.func.clone();
        for constant in &mut func.constants {
            if let Value::FunctionRef(h) = constant
                && let Some(j) = local.get(h)
            {
                *h = final_hashes[j];
            }
        }
        func.dependencies = func
            .dependencies
            .iter()
            .map(|dep| local.get(dep).map_or(*dep, |j| final_hashes[j]))
            .collect();
        func.hash = final_hash;

        result.functions.insert(final_hash, func);

        if let Some(parent) = &node.lambda_parent {
            result.lambda_parents.insert(final_hash, Arc::clone(parent));
        } else if let Some(name) = &node.name {
            result.function_names.insert(Arc::clone(name), final_hash);
            if node.is_main {
                result.entry_point = Some(final_hash);
            }
        }
    }

    Ok(result)
}

/// Finalize one recursive SCC as a group object.
fn finalize_group(
    scc_members: &[usize],
    member_set: &HashSet<usize>,
    nodes: &[Node],
    local: &HashMap<blake3::Hash, usize>,
    final_hashes: &mut HashMap<usize, blake3::Hash>,
    objects: &mut HashMap<blake3::Hash, StoredObject>,
) -> Result<(), CompileError> {
    let order = canonical_member_order(scc_members, member_set, nodes, local);

    let index_of: HashMap<usize, u32> = order
        .iter()
        .enumerate()
        .map(|(k, &i)| (i, k as u32))
        .collect();

    let subst = build_substitution(&order, member_set, nodes, local, final_hashes, &|j| {
        ObjectRef::Internal(index_of[&j])
    })?;

    let members = order
        .iter()
        .map(|&i| {
            Ok(GroupMember {
                name: nodes[i].name.as_ref().map(std::string::ToString::to_string),
                function: function_from_compiled(&nodes[i].func, &|h| resolve_ref(&subst, h))
                    .map_err(|_| {
                        internal_error("constant pool value cannot be content-addressed")
                    })?,
            })
        })
        .collect::<Result<Vec<_>, CompileError>>()?;

    let object = StoredObject::Group(members);
    let group_hash = object.hash();
    let count = order.len() as u32;

    for (k, &i) in order.iter().enumerate() {
        final_hashes.insert(i, member_hash(&group_hash, k as u32, count));
    }
    objects.insert(group_hash, object);

    // Multi-member groups also get redirect stubs so each member's hash
    // resolves to its group in the store.
    if count > 1 {
        for (k, &i) in order.iter().enumerate() {
            objects.insert(
                final_hashes[&i],
                StoredObject::Redirect {
                    group: group_hash,
                    index: k as u32,
                },
            );
        }
    }

    Ok(())
}

/// Canonical member ordering for a recursive group.
///
/// Named members first, sorted by name; then lambdas in the order they are
/// first referenced while scanning already-ordered members' constant pools.
/// Every lambda in a cycle is reachable from a named member of that cycle
/// (lambdas cannot recurse by name), so this covers all members; a trailing
/// temp-hash-ordered fallback guards against the impossible.
fn canonical_member_order(
    scc_members: &[usize],
    member_set: &HashSet<usize>,
    nodes: &[Node],
    local: &HashMap<blake3::Hash, usize>,
) -> Vec<usize> {
    let mut order: Vec<usize> = scc_members
        .iter()
        .copied()
        .filter(|&i| nodes[i].name.is_some())
        .collect();
    order.sort_by(|&a, &b| nodes[a].name.cmp(&nodes[b].name));

    let mut placed: HashSet<usize> = order.iter().copied().collect();
    let mut cursor = 0;
    while cursor < order.len() {
        let i = order[cursor];
        cursor += 1;
        for constant in &nodes[i].func.constants {
            if let Value::FunctionRef(h) = constant
                && let Some(&j) = local.get(h)
                && member_set.contains(&j)
                && placed.insert(j)
            {
                order.push(j);
            }
        }
    }

    let mut rest: Vec<usize> = scc_members
        .iter()
        .copied()
        .filter(|i| !placed.contains(i))
        .collect();
    debug_assert!(rest.is_empty(), "unreachable member in recursive group");
    rest.sort_by(|&a, &b| nodes[a].temp.as_bytes().cmp(nodes[b].temp.as_bytes()));
    order.extend(rest);

    order
}

/// Precompute the reference substitution for a set of functions being
/// encoded together: SCC-internal refs map via `internal`, refs to other
/// local functions map to their (already finalized) hashes, everything else
/// passes through as an external hash.
fn build_substitution(
    order: &[usize],
    member_set: &HashSet<usize>,
    nodes: &[Node],
    local: &HashMap<blake3::Hash, usize>,
    final_hashes: &HashMap<usize, blake3::Hash>,
    internal: &dyn Fn(usize) -> ObjectRef,
) -> Result<HashMap<blake3::Hash, ObjectRef>, CompileError> {
    let mut subst: HashMap<blake3::Hash, ObjectRef> = HashMap::new();
    for &i in order {
        let func = &nodes[i].func;
        let refs = func
            .constants
            .iter()
            .filter_map(|c| match c {
                Value::FunctionRef(h) => Some(*h),
                _ => None,
            })
            .chain(func.dependencies.iter().copied());
        for h in refs {
            if subst.contains_key(&h) {
                continue;
            }
            let resolved = match local.get(&h) {
                Some(&j) if member_set.contains(&j) => internal(j),
                Some(j) => ObjectRef::External(
                    *final_hashes
                        .get(j)
                        .ok_or_else(|| internal_error("dependency finalized out of order"))?,
                ),
                None => ObjectRef::External(h),
            };
            subst.insert(h, resolved);
        }
    }
    Ok(subst)
}

fn resolve_ref(subst: &HashMap<blake3::Hash, ObjectRef>, h: &blake3::Hash) -> ObjectRef {
    subst.get(h).copied().unwrap_or(ObjectRef::External(*h))
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
