//! Content-addressed store for compiled code.
//!
//! The store holds two views of the same code:
//!
//! - **Objects** ([`StoredObject`]) — the canonical, self-verifying encoding.
//!   An object's hash is the blake3 of its bytes, so objects can be
//!   persisted, transmitted, and re-verified anywhere. These are the unit of
//!   exchange (serialization, remote execution, disk).
//! - **Functions** ([`CompiledFunction`]) — the runnable view, materialized
//!   from objects. A plain object yields one function; a recursive group
//!   object yields one function per member.
//!
//! Functions added directly (without a canonical object, e.g. hand-built in
//! tests or REPL scratch code) are runnable but not portable: they cannot be
//! serialized or shipped, and [`Store::extract_pack`] reports them as
//! missing objects.
//!
//! # SCC Detection
//!
//! For mutually recursive functions, we use Tarjan's algorithm to detect
//! strongly connected components (SCCs). Functions in the same SCC are
//! hashed together as one group object.

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
// Pack length prefixes are u32 by design (canonical fixed-width encoding).
#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::object::{ObjectError, StoredObject};

/// Magic bytes identifying a pack (a batch of objects with roots).
pub const PACK_MAGIC: [u8; 4] = *b"ABPK";

/// Current pack encoding version.
pub const PACK_VERSION: u8 = 1;

/// A content-addressed store for compiled functions.
///
/// Functions are stored and retrieved by their content hash, enabling:
/// - Deduplication of identical functions
/// - Reliable dependency tracking
/// - Serialization for remote execution
#[derive(Debug, Default)]
pub struct Store {
    /// Function hash -> runnable function (materialized view).
    functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,
    /// Object hash -> canonical object (plain functions and groups).
    objects: HashMap<blake3::Hash, StoredObject>,
    /// Function hash -> hash of the object that provides it.
    providers: HashMap<blake3::Hash, blake3::Hash>,
}

impl Store {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a function to the store. Returns the hash.
    ///
    /// The function is runnable but has no canonical object, so it cannot be
    /// serialized or shipped. Prefer [`Store::add_object`] or
    /// [`Store::add_module`] for code that must be portable.
    pub fn add(&mut self, func: CompiledFunction) -> blake3::Hash {
        let hash = func.hash;
        self.functions.insert(hash, Arc::new(func));
        hash
    }

    /// Add a canonical object, materializing its function(s).
    ///
    /// Returns the object hash. Redirects are rejected: they are a
    /// disk-layout artifact, not portable content.
    pub fn add_object(&mut self, object: StoredObject) -> Result<blake3::Hash, StoreError> {
        let materialized = object.materialize().map_err(StoreError::Object)?;
        let object_hash = object.hash();
        for (func_hash, func) in materialized {
            // Keep an existing entry if present: it may carry debug info.
            self.functions
                .entry(func_hash)
                .or_insert_with(|| Arc::new(func));
            self.providers.insert(func_hash, object_hash);
        }
        self.objects.insert(object_hash, object);
        Ok(object_hash)
    }

    /// Add everything from a compiled module: canonical objects first, then
    /// the module's own functions (which may carry debug info) as the
    /// materialized view.
    pub fn add_module(&mut self, module: &crate::compiler::CompiledModule) {
        for object in module.objects.values() {
            if matches!(object, StoredObject::Redirect { .. }) {
                continue;
            }
            // Objects from a compiled module are well-formed by construction.
            let _ = self.add_object(object.clone());
        }
        for (hash, func) in &module.functions {
            self.functions.insert(*hash, Arc::new(func.clone()));
        }
    }

    /// Get the canonical object that provides a function.
    #[must_use]
    pub fn object_for(&self, func_hash: &blake3::Hash) -> Option<&StoredObject> {
        let object_hash = self.providers.get(func_hash)?;
        self.objects.get(object_hash)
    }

    /// All canonical objects in the store, keyed by object hash.
    #[must_use]
    pub fn objects(&self) -> &HashMap<blake3::Hash, StoredObject> {
        &self.objects
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

    /// Serialize every canonical object in the store as a pack.
    ///
    /// Functions without canonical objects are not included; use
    /// [`Store::extract_pack`] to fail loudly when a specific function must
    /// be shipped.
    pub fn serialize(&self) -> Result<Vec<u8>, StoreError> {
        let mut hashes: Vec<&blake3::Hash> = self.objects.keys().collect();
        hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let pack = Pack {
            entry_point: None,
            names: Vec::new(),
            objects: hashes.iter().map(|h| self.objects[*h].clone()).collect(),
        };
        Ok(pack.encode())
    }

    /// Deserialize a store from a pack.
    ///
    /// Every object is decoded from its canonical bytes, so hashes are
    /// recomputed (never trusted) and corruption is unrepresentable: a
    /// flipped bit yields a different object with a different hash.
    pub fn deserialize(data: &[u8]) -> Result<Self, StoreError> {
        let mut store = Store::new();
        store.add_pack(data)?;
        Ok(store)
    }

    /// Decode a pack and add every object in it. Redirects are skipped.
    pub fn add_pack(&mut self, data: &[u8]) -> Result<(), StoreError> {
        for object in Pack::decode(data)?.objects {
            if matches!(object, StoredObject::Redirect { .. }) {
                continue;
            }
            self.add_object(object)?;
        }
        Ok(())
    }

    /// Serialize a function and all its transitive dependencies as a pack.
    ///
    /// Fails if the function (or any dependency) has no canonical object.
    pub fn extract_pack(&self, hash: &blake3::Hash) -> Result<Vec<u8>, StoreError> {
        let subset = self.extract_with_dependencies(hash);
        let mut object_hashes: Vec<blake3::Hash> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut function_hashes: Vec<blake3::Hash> = subset.hashes();
        function_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for func_hash in function_hashes {
            let object_hash = self
                .providers
                .get(&func_hash)
                .ok_or(StoreError::MissingObject(func_hash))?;
            if seen.insert(*object_hash) {
                object_hashes.push(*object_hash);
            }
        }
        let pack = Pack {
            entry_point: None,
            names: Vec::new(),
            objects: object_hashes
                .iter()
                .filter_map(|h| self.objects.get(h).cloned())
                .collect(),
        };
        Ok(pack.encode())
    }

    /// Merge another store into this one.
    ///
    /// Functions and objects from the other store are added if they don't
    /// already exist.
    pub fn merge(&mut self, other: &Store) {
        for (hash, func) in &other.functions {
            if !self.contains(hash) {
                self.functions.insert(*hash, Arc::clone(func));
            }
        }
        for (hash, object) in &other.objects {
            self.objects.entry(*hash).or_insert_with(|| object.clone());
        }
        for (func_hash, object_hash) in &other.providers {
            self.providers.entry(*func_hash).or_insert(*object_hash);
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
            if let Some(object_hash) = self.providers.get(hash) {
                result.providers.insert(*hash, *object_hash);
                if let Some(object) = self.objects.get(object_hash) {
                    result
                        .objects
                        .entry(*object_hash)
                        .or_insert_with(|| object.clone());
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pack encoding
// ─────────────────────────────────────────────────────────────────────────────

/// A batch of canonical objects, optionally with an entry point and name
/// bindings — the unit of exchange between stores, over the wire, and the
/// content of `.ambient` artifact files.
///
/// Layout (integers little-endian):
///
/// ```text
/// "ABPK" | version u8
/// | has_entry u8 (0|1) | entry hash [32] (if has_entry)
/// | name_count u32 | names: (hash [32] | len u32 | utf8)*
/// | object_count u32 | objects: (len u32 | object bytes)*
/// ```
///
/// A wire pack (function shipping) carries no entry or names; an artifact
/// pack carries both so the program is runnable by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pack {
    /// The program entry point, if this pack is a runnable artifact.
    pub entry_point: Option<blake3::Hash>,
    /// Name → function-hash bindings.
    pub names: Vec<(String, blake3::Hash)>,
    /// The canonical objects.
    pub objects: Vec<StoredObject>,
}

impl Pack {
    /// Encode this pack to bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PACK_MAGIC);
        out.push(PACK_VERSION);
        match &self.entry_point {
            Some(hash) => {
                out.push(1);
                out.extend_from_slice(hash.as_bytes());
            }
            None => out.push(0),
        }
        out.extend_from_slice(&(self.names.len() as u32).to_le_bytes());
        for (name, hash) in &self.names {
            out.extend_from_slice(hash.as_bytes());
            out.extend_from_slice(&(name.len() as u32).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        out.extend_from_slice(&(self.objects.len() as u32).to_le_bytes());
        for object in &self.objects {
            let encoded = object.encode();
            out.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            out.extend_from_slice(&encoded);
        }
        out
    }

    /// Decode a pack from bytes.
    pub fn decode(data: &[u8]) -> Result<Self, StoreError> {
        let mut r = PackReader { data, pos: 0 };
        if r.take(4)? != PACK_MAGIC {
            return Err(StoreError::Deserialization(
                "not an Ambient pack (bad magic)".to_string(),
            ));
        }
        let version = r.u8()?;
        if version != PACK_VERSION {
            return Err(StoreError::Deserialization(format!(
                "unsupported pack version {version}"
            )));
        }

        let entry_point = match r.u8()? {
            0 => None,
            1 => Some(r.hash()?),
            t => {
                return Err(StoreError::Deserialization(format!(
                    "bad entry-point tag {t}"
                )))
            }
        };

        let name_count = r.u32()? as usize;
        let mut names = Vec::with_capacity(name_count.min(r.remaining()));
        for _ in 0..name_count {
            let hash = r.hash()?;
            let len = r.u32()? as usize;
            let raw = r.take(len)?;
            let name = std::str::from_utf8(raw)
                .map_err(|_| StoreError::Deserialization("name is not UTF-8".to_string()))?
                .to_string();
            names.push((name, hash));
        }

        let object_count = r.u32()? as usize;
        let mut objects = Vec::with_capacity(object_count.min(r.remaining()));
        for _ in 0..object_count {
            let len = r.u32()? as usize;
            let raw = r.take(len)?;
            objects.push(StoredObject::decode(raw).map_err(StoreError::Object)?);
        }

        if r.pos != data.len() {
            return Err(StoreError::Deserialization(
                "trailing bytes after pack".to_string(),
            ));
        }

        Ok(Self {
            entry_point,
            names,
            objects,
        })
    }
}

struct PackReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PackReader<'a> {
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| StoreError::Deserialization("pack is truncated".to_string()))?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, StoreError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn hash(&mut self) -> Result<blake3::Hash, StoreError> {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(self.take(32)?);
        Ok(blake3::Hash::from_bytes(bytes))
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Serialization failed.
    Serialization(String),
    /// Deserialization failed.
    Deserialization(String),
    /// A canonical object was malformed.
    Object(ObjectError),
    /// A function has no canonical object and cannot be shipped.
    MissingObject(blake3::Hash),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Deserialization(msg) => write!(f, "deserialization error: {msg}"),
            Self::Object(e) => write!(f, "object error: {e}"),
            Self::MissingObject(hash) => {
                write!(f, "function {hash} has no canonical object; cannot ship it")
            }
        }
    }
}

impl std::error::Error for StoreError {}

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

    /// Create a canonical plain object for serialization testing.
    fn make_plain_object(return_value: f64) -> StoredObject {
        use crate::object::{ObjectConstant, ObjectFunction};
        StoredObject::Plain(ObjectFunction {
            bytecode: vec![1, 2, 3],
            constants: vec![ObjectConstant::Number(return_value)],
            local_count: 0,
            param_count: 0,
            dependencies: vec![],
        })
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

        let object = make_plain_object(42.0);
        let hash = store.add_object(object).expect("add_object failed");

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
        use crate::object::{ObjectConstant, ObjectFunction, ObjectRef};

        let mut store = Store::new();

        let dep_object = make_plain_object(1.0);
        let dep_hash = store.add_object(dep_object).expect("add dep failed");

        let main_object = StoredObject::Plain(ObjectFunction {
            bytecode: vec![9, 9],
            constants: vec![ObjectConstant::Ref(ObjectRef::External(dep_hash))],
            local_count: 0,
            param_count: 0,
            dependencies: vec![ObjectRef::External(dep_hash)],
        });
        let main_hash = store.add_object(main_object).expect("add main failed");

        // Serialize and deserialize
        let data = store.serialize().expect("serialization failed");
        let store2 = Store::deserialize(&data).expect("deserialization failed");

        assert_eq!(store2.len(), 2);
        assert!(store2.contains(&dep_hash));
        assert!(store2.contains(&main_hash));

        let main2 = store2.get(&main_hash).expect("main function not found");
        assert_eq!(main2.dependencies, vec![dep_hash]);
    }

    #[test]
    fn test_recursive_group_survives_roundtrip() {
        use crate::object::{member_hash, GroupMember, ObjectConstant, ObjectFunction, ObjectRef};

        // Mutually recursive pair, stored as one group object.
        let make_member = |name: &str, other: u32| GroupMember {
            name: Some(name.to_string()),
            function: ObjectFunction {
                bytecode: vec![7],
                constants: vec![ObjectConstant::Ref(ObjectRef::Internal(other))],
                local_count: 0,
                param_count: 1,
                dependencies: vec![ObjectRef::Internal(other)],
            },
        };
        let group = StoredObject::Group(vec![make_member("even", 1), make_member("odd", 0)]);
        let group_hash = group.hash();

        let mut store = Store::new();
        store.add_object(group).expect("add group failed");

        let even_hash = member_hash(&group_hash, 0, 2);
        let odd_hash = member_hash(&group_hash, 1, 2);
        assert!(store.contains(&even_hash));
        assert!(store.contains(&odd_hash));

        // The old JSON format failed hash verification for recursive
        // functions (member hashes are not recomputable from a single
        // function). The pack format ships the group object, so recursion
        // survives serialization.
        let data = store.extract_pack(&even_hash).expect("extract failed");
        let store2 = Store::deserialize(&data).expect("deserialize failed");
        assert!(store2.contains(&even_hash));
        assert!(store2.contains(&odd_hash));
        assert_eq!(
            store2.get(&even_hash).expect("even missing").dependencies,
            vec![odd_hash]
        );
    }

    #[test]
    fn test_extract_pack_rejects_unportable_function() {
        let mut store = Store::new();
        let func = make_test_function("test::scratch", 1.0);
        let hash = func.hash;
        store.add(func);

        assert!(matches!(
            store.extract_pack(&hash),
            Err(StoreError::MissingObject(h)) if h == hash
        ));
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
