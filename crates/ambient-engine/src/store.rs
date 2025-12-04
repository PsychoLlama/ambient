//! Content-addressed store for compiled functions.
//!
//! This module provides a content-addressed storage system where functions are
//! identified by the hash of their implementation and type signature.
//!
//! # SCC Detection
//!
//! For mutually recursive functions, we use Tarjan's algorithm to detect strongly
//! connected components (SCCs). Functions in the same SCC are hashed together
//! to ensure consistent identification.

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::bytecode::CompiledFunction;

/// A content-addressed store for compiled functions.
///
/// Functions are stored and retrieved by their content hash, enabling:
/// - Deduplication of identical functions
/// - Reliable dependency tracking
/// - Serialization for remote execution
#[derive(Debug, Default)]
pub struct Store {
    /// Hash -> compiled function
    functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,
}

impl Store {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a function to the store. Returns the hash.
    pub fn add(&mut self, func: CompiledFunction) -> blake3::Hash {
        let hash = func.hash;
        self.functions.insert(hash, Arc::new(func));
        hash
    }

    /// Get a function by its hash.
    #[must_use]
    pub fn get(&self, hash: &blake3::Hash) -> Option<Arc<CompiledFunction>> {
        self.functions.get(hash).cloned()
    }

    /// Check if a function exists in the store.
    #[must_use]
    pub fn contains(&self, hash: &blake3::Hash) -> bool {
        self.functions.contains_key(hash)
    }

    /// Get all function hashes in the store.
    #[must_use]
    pub fn hashes(&self) -> Vec<blake3::Hash> {
        self.functions.keys().copied().collect()
    }

    /// Get the number of functions in the store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Check if the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Find missing dependencies for a function.
    ///
    /// Returns a list of hashes that the function depends on but are not in the store.
    #[must_use]
    pub fn missing_dependencies(&self, hash: &blake3::Hash) -> Vec<blake3::Hash> {
        let Some(func) = self.get(hash) else {
            return vec![];
        };

        func.dependencies
            .iter()
            .filter(|dep| !self.contains(dep))
            .copied()
            .collect()
    }

    /// Get all dependencies of a function (transitively).
    ///
    /// Returns all functions that are reachable from the given function.
    #[must_use]
    pub fn transitive_dependencies(&self, hash: &blake3::Hash) -> Vec<blake3::Hash> {
        let mut visited = std::collections::HashSet::new();
        let mut result = Vec::new();
        self.collect_dependencies(hash, &mut visited, &mut result);
        result
    }

    fn collect_dependencies(
        &self,
        hash: &blake3::Hash,
        visited: &mut std::collections::HashSet<blake3::Hash>,
        result: &mut Vec<blake3::Hash>,
    ) {
        if !visited.insert(*hash) {
            return; // Already visited
        }

        if let Some(func) = self.get(hash) {
            for dep in &func.dependencies {
                if !visited.contains(dep) {
                    result.push(*dep);
                    self.collect_dependencies(dep, visited, result);
                }
            }
        }
    }

    /// Serialize the store to a portable format.
    pub fn serialize(&self) -> Result<Vec<u8>, StoreError> {
        let portable: PortableStore = self.into();
        serde_json::to_vec(&portable).map_err(|e| StoreError::Serialization(e.to_string()))
    }

    /// Deserialize a store from a portable format.
    pub fn deserialize(data: &[u8]) -> Result<Self, StoreError> {
        let portable: PortableStore =
            serde_json::from_slice(data).map_err(|e| StoreError::Deserialization(e.to_string()))?;
        portable.try_into()
    }

    /// Merge another store into this one.
    ///
    /// Functions from the other store are added if they don't already exist.
    pub fn merge(&mut self, other: &Store) {
        for (hash, func) in &other.functions {
            if !self.contains(hash) {
                self.functions.insert(*hash, Arc::clone(func));
            }
        }
    }

    /// Extract a subset of the store containing the given function and all its dependencies.
    #[must_use]
    pub fn extract_with_dependencies(&self, hash: &blake3::Hash) -> Store {
        let mut result = Store::new();
        let mut visited = std::collections::HashSet::new();
        self.extract_recursive(hash, &mut visited, &mut result);
        result
    }

    fn extract_recursive(
        &self,
        hash: &blake3::Hash,
        visited: &mut std::collections::HashSet<blake3::Hash>,
        result: &mut Store,
    ) {
        if !visited.insert(*hash) {
            return;
        }

        if let Some(func) = self.get(hash) {
            // First extract dependencies
            for dep in &func.dependencies {
                self.extract_recursive(dep, visited, result);
            }
            // Then add the function itself
            result.functions.insert(*hash, Arc::clone(&func));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SCC Detection (Tarjan's Algorithm)
// ─────────────────────────────────────────────────────────────────────────────

/// Strongly Connected Component - a group of nodes in a directed graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericScc<T> {
    /// The nodes in this SCC (sorted for deterministic ordering).
    pub members: Vec<T>,
}

impl<T> GenericScc<T> {
    /// Returns true if this SCC represents a single node.
    #[must_use]
    pub fn is_singleton(&self) -> bool {
        self.members.len() == 1
    }

    /// Returns the number of nodes in this SCC.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Returns true if the SCC is empty (should not happen in practice).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Result of SCC analysis on a directed graph.
#[derive(Debug, Clone)]
pub struct GenericSccAnalysis<T: std::hash::Hash + Eq + Clone> {
    /// All strongly connected components, in reverse topological order.
    /// (Components that depend on nothing come first.)
    pub components: Vec<GenericScc<T>>,
    /// Map from node to its SCC index.
    pub node_to_scc: HashMap<T, usize>,
}

impl<T: std::hash::Hash + Eq + Clone> GenericSccAnalysis<T> {
    /// Get the SCC containing a specific node.
    #[must_use]
    pub fn scc_for(&self, node: &T) -> Option<&GenericScc<T>> {
        self.node_to_scc.get(node).map(|&idx| &self.components[idx])
    }

    /// Returns true if a node is part of a non-trivial cycle (multi-node SCC).
    #[must_use]
    pub fn is_in_cycle(&self, node: &T) -> bool {
        self.scc_for(node).is_some_and(|scc| scc.members.len() > 1)
    }
}

/// Compute strongly connected components from a directed graph using Tarjan's algorithm.
///
/// Returns SCCs in reverse topological order (dependencies before dependents).
///
/// # Arguments
/// * `graph` - A map from each node to the nodes it points to (successors)
///
/// # Type Parameters
/// * `T` - Node type, must be hashable and cloneable. If `T: Ord`, results are deterministically sorted.
pub fn compute_sccs<T, S: std::hash::BuildHasher>(
    graph: &HashMap<T, Vec<T>, S>,
) -> GenericSccAnalysis<T>
where
    T: std::hash::Hash + Eq + Clone,
{
    compute_sccs_with_cmp(graph, |a, b| {
        // Default comparison using hash of debug representation for types without Ord
        // This provides deterministic ordering without requiring Ord
        use std::hash::Hasher;
        let hash_of = |x: &T| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            x.hash(&mut h);
            h.finish()
        };
        hash_of(a).cmp(&hash_of(b))
    })
}

/// Compute SCCs with a custom comparison function for deterministic ordering.
pub fn compute_sccs_with_cmp<T, F, S: std::hash::BuildHasher>(
    graph: &HashMap<T, Vec<T>, S>,
    cmp: F,
) -> GenericSccAnalysis<T>
where
    T: std::hash::Hash + Eq + Clone,
    F: Fn(&T, &T) -> std::cmp::Ordering + Copy,
{
    struct TarjanState<T> {
        index_counter: usize,
        indices: HashMap<T, usize>,
        lowlinks: HashMap<T, usize>,
        on_stack: HashMap<T, bool>,
        stack: Vec<T>,
        sccs: Vec<GenericScc<T>>,
    }

    fn visit<T, F, S: std::hash::BuildHasher>(
        node: &T,
        graph: &HashMap<T, Vec<T>, S>,
        state: &mut TarjanState<T>,
        cmp: F,
    ) where
        T: std::hash::Hash + Eq + Clone,
        F: Fn(&T, &T) -> std::cmp::Ordering + Copy,
    {
        let index = state.index_counter;
        state.indices.insert(node.clone(), index);
        state.lowlinks.insert(node.clone(), index);
        state.index_counter += 1;
        state.stack.push(node.clone());
        state.on_stack.insert(node.clone(), true);

        if let Some(successors) = graph.get(node) {
            for successor in successors {
                if !state.indices.contains_key(successor) {
                    visit(successor, graph, state, cmp);
                    let succ_lowlink = state.lowlinks.get(successor).copied().unwrap_or(usize::MAX);
                    let curr_lowlink = state.lowlinks.get(node).copied().unwrap_or(usize::MAX);
                    state
                        .lowlinks
                        .insert(node.clone(), curr_lowlink.min(succ_lowlink));
                } else if state.on_stack.get(successor).copied().unwrap_or(false) {
                    let succ_index = state.indices.get(successor).copied().unwrap_or(usize::MAX);
                    let curr_lowlink = state.lowlinks.get(node).copied().unwrap_or(usize::MAX);
                    state
                        .lowlinks
                        .insert(node.clone(), curr_lowlink.min(succ_index));
                }
            }
        }

        if state.lowlinks.get(node) == state.indices.get(node) {
            let mut members = Vec::new();
            while let Some(w) = state.stack.pop() {
                state.on_stack.insert(w.clone(), false);
                let is_node = &w == node;
                members.push(w);
                if is_node {
                    break;
                }
            }
            // Sort members for deterministic ordering
            members.sort_by(cmp);
            state.sccs.push(GenericScc { members });
        }
    }

    let mut state = TarjanState {
        index_counter: 0,
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        on_stack: HashMap::new(),
        stack: Vec::new(),
        sccs: Vec::new(),
    };

    // Visit nodes in deterministic order
    let mut nodes: Vec<_> = graph.keys().cloned().collect();
    nodes.sort_by(&cmp);
    for node in &nodes {
        if !state.indices.contains_key(node) {
            visit(node, graph, &mut state, cmp);
        }
    }

    // Build node_to_scc map
    let mut node_to_scc = HashMap::new();
    for (scc_idx, scc) in state.sccs.iter().enumerate() {
        for node in &scc.members {
            node_to_scc.insert(node.clone(), scc_idx);
        }
    }

    GenericSccAnalysis {
        components: state.sccs,
        node_to_scc,
    }
}

// Type aliases for backward compatibility with existing code
/// Strongly Connected Component for function hashes.
pub type Scc = GenericScc<blake3::Hash>;

/// SCC analysis result for function hashes.
pub type SccAnalysis = GenericSccAnalysis<blake3::Hash>;

impl SccAnalysis {
    /// Returns true if a function is part of a non-trivial cycle.
    /// (Alias for `is_in_cycle` for backward compatibility)
    #[must_use]
    pub fn is_recursive(&self, hash: &blake3::Hash) -> bool {
        self.is_in_cycle(hash)
    }
}

impl Store {
    /// Compute strongly connected components for all functions in the store.
    ///
    /// Uses Tarjan's algorithm to find groups of mutually recursive functions.
    /// Returns SCCs in reverse topological order (dependencies before dependents).
    #[must_use]
    pub fn compute_sccs(&self) -> SccAnalysis {
        // Build call graph from store
        let mut call_graph: HashMap<blake3::Hash, Vec<blake3::Hash>> = HashMap::new();
        for (hash, func) in &self.functions {
            call_graph.insert(*hash, func.dependencies.clone());
        }

        // Use the generic SCC algorithm with byte comparison for blake3::Hash
        compute_sccs_with_cmp(&call_graph, |a, b| a.as_bytes().cmp(b.as_bytes()))
    }

    /// Check if a function is part of a recursive cycle.
    #[must_use]
    pub fn is_recursive(&self, hash: &blake3::Hash) -> bool {
        let analysis = self.compute_sccs();
        analysis.is_recursive(hash)
    }

    /// Get all functions that are mutually recursive with the given function.
    #[must_use]
    pub fn mutual_recursion_group(&self, hash: &blake3::Hash) -> Option<Vec<blake3::Hash>> {
        let analysis = self.compute_sccs();
        analysis.scc_for(hash).map(|scc| scc.members.clone())
    }
}

/// Error type for store operations.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreError {
    /// Serialization failed.
    Serialization(String),
    /// Deserialization failed.
    Deserialization(String),
    /// Hash mismatch during deserialization.
    HashMismatch {
        expected: blake3::Hash,
        computed: blake3::Hash,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Deserialization(msg) => write!(f, "deserialization error: {msg}"),
            Self::HashMismatch { expected, computed } => {
                write!(f, "hash mismatch: expected {expected}, computed {computed}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

/// Portable representation of a store for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableStore {
    /// Version number for format compatibility.
    pub version: u32,
    /// The functions in the store.
    pub functions: Vec<PortableFunction>,
}

impl From<&Store> for PortableStore {
    fn from(store: &Store) -> Self {
        Self {
            version: 1,
            functions: store
                .functions
                .values()
                .map(|f| PortableFunction::from(f.as_ref()))
                .collect(),
        }
    }
}

impl TryFrom<PortableStore> for Store {
    type Error = StoreError;

    fn try_from(portable: PortableStore) -> Result<Self, Self::Error> {
        let mut store = Store::new();
        for pf in portable.functions {
            let func = CompiledFunction::try_from(pf)?;
            store.add(func);
        }
        Ok(store)
    }
}

/// Portable representation of a compiled function for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableFunction {
    /// The content hash (hex-encoded for JSON compatibility).
    pub hash: String,
    /// The bytecode as base64.
    pub bytecode: String,
    /// Constants as JSON values.
    pub constants: Vec<crate::value::Value>,
    /// Number of local slots.
    pub local_count: u16,
    /// Number of parameters.
    pub param_count: u8,
    /// Dependencies (hex-encoded hashes).
    pub dependencies: Vec<String>,
}

impl From<&CompiledFunction> for PortableFunction {
    fn from(func: &CompiledFunction) -> Self {
        use base64::Engine;
        Self {
            hash: func.hash.to_hex().to_string(),
            bytecode: base64::engine::general_purpose::STANDARD.encode(&func.bytecode),
            constants: func.constants.clone(),
            local_count: func.local_count,
            param_count: func.param_count,
            dependencies: func
                .dependencies
                .iter()
                .map(|h| h.to_hex().to_string())
                .collect(),
        }
    }
}

impl TryFrom<PortableFunction> for CompiledFunction {
    type Error = StoreError;

    fn try_from(pf: PortableFunction) -> Result<Self, Self::Error> {
        use base64::Engine;

        // Decode bytecode
        let bytecode = base64::engine::general_purpose::STANDARD
            .decode(&pf.bytecode)
            .map_err(|e| StoreError::Deserialization(format!("invalid base64: {e}")))?;

        // Decode dependencies
        let deps: Vec<blake3::Hash> = pf
            .dependencies
            .iter()
            .map(|s| parse_hash(s))
            .collect::<Result<Vec<_>, _>>()?;

        // Create function with computed hash
        let func = CompiledFunction::with_dependencies(
            bytecode,
            pf.constants,
            pf.local_count,
            pf.param_count,
            deps,
        );

        // Verify hash matches
        let expected = parse_hash(&pf.hash)?;
        if func.hash != expected {
            return Err(StoreError::HashMismatch {
                expected,
                computed: func.hash,
            });
        }

        Ok(func)
    }
}

/// Parse a hex-encoded hash string.
fn parse_hash(s: &str) -> Result<blake3::Hash, StoreError> {
    let bytes =
        hex::decode(s).map_err(|e| StoreError::Deserialization(format!("invalid hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(StoreError::Deserialization(format!(
            "invalid hash length: expected 32, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(blake3::Hash::from_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::BytecodeBuilder;
    use crate::bytecode::Opcode;
    use crate::value::Value;

    /// Create a function with a predictable hash for testing.
    /// The hash is overridden to allow testing without content-address coupling.
    fn make_test_function(name: &str, return_value: f64) -> CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(return_value));
        builder.emit(Opcode::Return);
        let mut func = builder.build(0, 0);
        // Give it a predictable hash based on name for testing
        func.hash = blake3::hash(name.as_bytes());
        func
    }

    /// Create a function with a content-derived hash for serialization testing.
    fn make_content_addressed_function(return_value: f64) -> CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(return_value));
        builder.emit(Opcode::Return);
        builder.build(0, 0)
    }

    #[test]
    fn test_store_add_and_get() {
        let mut store = Store::new();
        let func = make_test_function("test::foo", 42.0);
        let hash = func.hash;

        store.add(func);

        assert!(store.contains(&hash));
        assert!(store.get(&hash).is_some());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_store_missing_dependencies() {
        let mut store = Store::new();

        // Create a function with dependencies
        let dep_hash = blake3::hash(b"test::dependency");
        let mut func = make_test_function("test::caller", 1.0);
        func.dependencies = vec![dep_hash];
        let caller_hash = func.hash;

        store.add(func);

        // Dependency is missing
        let missing = store.missing_dependencies(&caller_hash);
        assert_eq!(missing, vec![dep_hash]);

        // Add the dependency
        let mut dep_func = make_test_function("test::dep_impl", 2.0);
        dep_func.hash = dep_hash;
        store.add(dep_func);

        // No longer missing
        let missing = store.missing_dependencies(&caller_hash);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_store_transitive_dependencies() {
        let mut store = Store::new();

        // Create chain: A -> B -> C
        let hash_c = blake3::hash(b"test::c");
        let hash_b = blake3::hash(b"test::b");
        let hash_a = blake3::hash(b"test::a");

        let mut func_c = make_test_function("c_impl", 1.0);
        func_c.hash = hash_c;
        func_c.dependencies = vec![];

        let mut func_b = make_test_function("b_impl", 2.0);
        func_b.hash = hash_b;
        func_b.dependencies = vec![hash_c];

        let mut func_a = make_test_function("a_impl", 3.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_b];

        store.add(func_c);
        store.add(func_b);
        store.add(func_a);

        let deps = store.transitive_dependencies(&hash_a);
        assert!(deps.contains(&hash_b));
        assert!(deps.contains(&hash_c));
    }

    #[test]
    fn test_store_extract_with_dependencies() {
        let mut store = Store::new();

        let hash_dep = blake3::hash(b"test::dep");
        let hash_main = blake3::hash(b"test::main");
        let hash_other = blake3::hash(b"test::other");

        let mut func_dep = make_test_function("dep", 1.0);
        func_dep.hash = hash_dep;

        let mut func_main = make_test_function("main", 2.0);
        func_main.hash = hash_main;
        func_main.dependencies = vec![hash_dep];

        let mut func_other = make_test_function("other", 3.0);
        func_other.hash = hash_other;

        store.add(func_dep);
        store.add(func_main);
        store.add(func_other);

        // Extract main and its deps, should not include "other"
        let extracted = store.extract_with_dependencies(&hash_main);
        assert_eq!(extracted.len(), 2);
        assert!(extracted.contains(&hash_main));
        assert!(extracted.contains(&hash_dep));
        assert!(!extracted.contains(&hash_other));
    }

    #[test]
    fn test_store_merge() {
        let mut store1 = Store::new();
        let mut store2 = Store::new();

        let func1 = make_test_function("test::a", 1.0);
        let hash1 = func1.hash;
        store1.add(func1);

        let func2 = make_test_function("test::b", 2.0);
        let hash2 = func2.hash;
        store2.add(func2);

        store1.merge(&store2);

        assert_eq!(store1.len(), 2);
        assert!(store1.contains(&hash1));
        assert!(store1.contains(&hash2));
    }

    #[test]
    fn test_store_serialize_roundtrip() {
        let mut store = Store::new();

        // Use content-addressed function for serialization test
        let func = make_content_addressed_function(42.0);
        let hash = func.hash;
        store.add(func);

        // Serialize
        let data = store.serialize().expect("serialization failed");

        // Deserialize
        let store2 = Store::deserialize(&data).expect("deserialization failed");

        assert_eq!(store2.len(), 1);
        assert!(store2.contains(&hash));

        let func2 = store2.get(&hash).expect("function not found");
        assert_eq!(func2.constants.len(), 1);
        assert_eq!(func2.hash, hash);
    }

    #[test]
    fn test_store_serialize_with_dependencies() {
        let mut store = Store::new();

        // Create two content-addressed functions with a dependency relationship
        let dep_func = make_content_addressed_function(1.0);
        let dep_hash = dep_func.hash;

        let mut main_builder = BytecodeBuilder::new();
        main_builder.emit_call(dep_hash, 0);
        main_builder.emit(Opcode::Return);
        let main_func = main_builder.build_with_dependencies(0, 0, vec![dep_hash]);
        let main_hash = main_func.hash;

        store.add(dep_func);
        store.add(main_func);

        // Serialize and deserialize
        let data = store.serialize().expect("serialization failed");
        let store2 = Store::deserialize(&data).expect("deserialization failed");

        assert_eq!(store2.len(), 2);
        assert!(store2.contains(&dep_hash));
        assert!(store2.contains(&main_hash));

        let main2 = store2.get(&main_hash).expect("main function not found");
        assert_eq!(main2.dependencies, vec![dep_hash]);
    }

    // =========================================================================
    // SCC Detection Tests
    // =========================================================================

    #[test]
    fn test_scc_single_function() {
        let mut store = Store::new();
        let func = make_test_function("test::single", 42.0);
        let hash = func.hash;
        store.add(func);

        let analysis = store.compute_sccs();
        assert_eq!(analysis.components.len(), 1);
        assert!(analysis.components[0].is_singleton());
        assert_eq!(analysis.components[0].members, vec![hash]);
    }

    #[test]
    fn test_scc_linear_chain() {
        let mut store = Store::new();

        // Create A -> B -> C (no cycles)
        let hash_c = blake3::hash(b"test::c");
        let hash_b = blake3::hash(b"test::b");
        let hash_a = blake3::hash(b"test::a");

        let mut func_c = make_test_function("c", 1.0);
        func_c.hash = hash_c;
        func_c.dependencies = vec![];

        let mut func_b = make_test_function("b", 2.0);
        func_b.hash = hash_b;
        func_b.dependencies = vec![hash_c];

        let mut func_a = make_test_function("a", 3.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_b];

        store.add(func_c);
        store.add(func_b);
        store.add(func_a);

        let analysis = store.compute_sccs();
        // Each function is its own SCC (no cycles)
        assert_eq!(analysis.components.len(), 3);
        for scc in &analysis.components {
            assert!(scc.is_singleton());
        }
        assert!(!analysis.is_recursive(&hash_a));
        assert!(!analysis.is_recursive(&hash_b));
        assert!(!analysis.is_recursive(&hash_c));
    }

    #[test]
    fn test_scc_mutual_recursion() {
        let mut store = Store::new();

        // Create A <-> B (mutual recursion)
        let hash_a = blake3::hash(b"test::a");
        let hash_b = blake3::hash(b"test::b");

        let mut func_a = make_test_function("a", 1.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_b];

        let mut func_b = make_test_function("b", 2.0);
        func_b.hash = hash_b;
        func_b.dependencies = vec![hash_a];

        store.add(func_a);
        store.add(func_b);

        let analysis = store.compute_sccs();
        // Both functions are in the same SCC
        assert_eq!(analysis.components.len(), 1);
        assert_eq!(analysis.components[0].members.len(), 2);
        assert!(analysis.components[0].members.contains(&hash_a));
        assert!(analysis.components[0].members.contains(&hash_b));
        assert!(analysis.is_recursive(&hash_a));
        assert!(analysis.is_recursive(&hash_b));
    }

    #[test]
    fn test_scc_self_recursion() {
        let mut store = Store::new();

        // Create A -> A (self recursion)
        let hash_a = blake3::hash(b"test::a");

        let mut func_a = make_test_function("a", 1.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_a]; // Calls itself

        store.add(func_a);

        let analysis = store.compute_sccs();
        // Self-recursive function is its own SCC but is_recursive returns false
        // because it's a singleton (self-recursion is handled separately)
        assert_eq!(analysis.components.len(), 1);
        assert!(analysis.components[0].is_singleton());
    }

    #[test]
    fn test_scc_complex_graph() {
        let mut store = Store::new();

        // Create a graph with multiple SCCs:
        //   A -> B -> C -> D
        //        ^    |
        //        +----+
        //   E (standalone)
        let hash_a = blake3::hash(b"test::a");
        let hash_b = blake3::hash(b"test::b");
        let hash_c = blake3::hash(b"test::c");
        let hash_d = blake3::hash(b"test::d");
        let hash_e = blake3::hash(b"test::e");

        let mut func_a = make_test_function("a", 1.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_b];

        let mut func_b = make_test_function("b", 2.0);
        func_b.hash = hash_b;
        func_b.dependencies = vec![hash_c];

        let mut func_c = make_test_function("c", 3.0);
        func_c.hash = hash_c;
        func_c.dependencies = vec![hash_b, hash_d]; // C -> B creates cycle, C -> D

        let mut func_d = make_test_function("d", 4.0);
        func_d.hash = hash_d;
        func_d.dependencies = vec![];

        let mut func_e = make_test_function("e", 5.0);
        func_e.hash = hash_e;
        func_e.dependencies = vec![];

        store.add(func_a);
        store.add(func_b);
        store.add(func_c);
        store.add(func_d);
        store.add(func_e);

        let analysis = store.compute_sccs();

        // B and C form an SCC (mutual recursion)
        // A, D, E are each their own SCC
        assert_eq!(analysis.components.len(), 4);

        // B and C should be in the same SCC
        let bc_scc = analysis.scc_for(&hash_b).expect("B should have an SCC");
        assert_eq!(bc_scc.members.len(), 2);
        assert!(bc_scc.members.contains(&hash_b));
        assert!(bc_scc.members.contains(&hash_c));

        // A, D, E should each be singletons
        assert!(analysis
            .scc_for(&hash_a)
            .is_some_and(|scc| scc.is_singleton()));
        assert!(analysis
            .scc_for(&hash_d)
            .is_some_and(|scc| scc.is_singleton()));
        assert!(analysis
            .scc_for(&hash_e)
            .is_some_and(|scc| scc.is_singleton()));
    }

    #[test]
    fn test_mutual_recursion_group() {
        let mut store = Store::new();

        let hash_a = blake3::hash(b"test::a");
        let hash_b = blake3::hash(b"test::b");

        let mut func_a = make_test_function("a", 1.0);
        func_a.hash = hash_a;
        func_a.dependencies = vec![hash_b];

        let mut func_b = make_test_function("b", 2.0);
        func_b.hash = hash_b;
        func_b.dependencies = vec![hash_a];

        store.add(func_a);
        store.add(func_b);

        let group = store.mutual_recursion_group(&hash_a);
        assert!(group.is_some());
        let group = group.expect("group should exist");
        assert_eq!(group.len(), 2);
        assert!(group.contains(&hash_a));
        assert!(group.contains(&hash_b));
    }
}
