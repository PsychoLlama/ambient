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
use crate::value::Value;

/// Magic bytes identifying a pack (a batch of objects with roots).
pub const PACK_MAGIC: [u8; 4] = *b"ABPK";

/// Current pack encoding version.
pub const PACK_VERSION: u8 = 2;

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
    /// Object hash -> canonical object (plain functions, groups, and values).
    objects: HashMap<blake3::Hash, StoredObject>,
    /// Function hash -> hash of the object that provides it.
    providers: HashMap<blake3::Hash, blake3::Hash>,
    /// Value-object hash -> the `const` value it holds (materialized view).
    values: HashMap<blake3::Hash, Value>,
    /// Native-object hash -> its `(uuid, param_count)` identity
    /// (materialized view). The VM binds the uuid against its native table.
    natives: HashMap<blake3::Hash, (uuid::Uuid, u8)>,
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
        let object_hash = object.hash();
        // A value object holds a `const`, not code: index it into `values`
        // (keyed by its own content hash) rather than materializing functions.
        if let Some(value) = object.as_value() {
            self.values.entry(object_hash).or_insert(value);
            self.objects.insert(object_hash, object);
            return Ok(object_hash);
        }
        // A native object holds an extern fn's identity, not bytecode: index
        // it so a VM can bind the uuid. It provides "itself" — a caller's
        // dependency on the native hash resolves to this object for shipping.
        if let Some((uuid, param_count)) = object.as_native() {
            self.natives.insert(object_hash, (uuid, param_count));
            self.providers.insert(object_hash, object_hash);
            self.objects.insert(object_hash, object);
            return Ok(object_hash);
        }
        let materialized = object.materialize().map_err(StoreError::Object)?;
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

    /// Get a `const` value object by its content hash.
    #[must_use]
    pub fn get_value(&self, hash: &blake3::Hash) -> Option<&Value> {
        self.values.get(hash)
    }

    /// Every `const` value object in the store, keyed by content hash.
    #[must_use]
    pub fn values(&self) -> &HashMap<blake3::Hash, Value> {
        &self.values
    }

    /// Every native (extern fn) object in the store, keyed by content hash.
    #[must_use]
    pub fn natives(&self) -> &HashMap<blake3::Hash, (uuid::Uuid, u8)> {
        &self.natives
    }

    /// Check if a hash resolves to code or data in the store: a function,
    /// a `const` value object, or a native (extern fn). A dependency edge
    /// can point at any of the three, so presence checks
    /// ([`Self::missing_dependencies`], the Execute ability's
    /// `has_function`) must cover them all — a native reported "missing"
    /// would make a remote client re-ship it forever.
    #[must_use]
    pub fn contains(&self, hash: &blake3::Hash) -> bool {
        self.functions.contains_key(hash)
            || self.values.contains_key(hash)
            || self.natives.contains_key(hash)
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
            for dep in func.referenced_hashes() {
                if !visited.contains(&dep) {
                    result.push(dep);
                    self.collect_dependencies(&dep, visited, result);
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
            objects: hashes.iter().map(|h| self.objects[*h].clone()).collect(),
            ..Pack::default()
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
        // Value objects a function depends on are content-addressed by their
        // own hash (they have no provider entry); include them so a shipped
        // closure carries the `const`s it reads.
        let mut value_hashes: Vec<blake3::Hash> = subset.values.keys().copied().collect();
        value_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for value_hash in value_hashes {
            if seen.insert(value_hash) {
                object_hashes.push(value_hash);
            }
        }
        // Native objects likewise: leaves addressed by their own hash,
        // shipped so the receiver can bind (or loudly reject) the uuid.
        let mut native_hashes: Vec<blake3::Hash> = subset.natives.keys().copied().collect();
        native_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for native_hash in native_hashes {
            if seen.insert(native_hash) {
                object_hashes.push(native_hash);
            }
        }
        let pack = Pack {
            objects: object_hashes
                .iter()
                .filter_map(|h| self.objects.get(h).cloned())
                .collect(),
            ..Pack::default()
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
        for (hash, value) in &other.values {
            self.values.entry(*hash).or_insert_with(|| value.clone());
        }
        for (hash, native) in &other.natives {
            self.natives.entry(*hash).or_insert(*native);
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
            // First extract everything referenced (recorded dependencies
            // plus bare constant-pool refs — see `referenced_hashes`).
            let referenced: Vec<blake3::Hash> = func.referenced_hashes().collect();
            for dep in referenced {
                self.extract_recursive(&dep, visited, result);
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
        } else if let Some(value) = self.values.get(hash) {
            // A `const` value object a function depends on: a leaf, so there
            // is nothing further to recurse into.
            result.values.entry(*hash).or_insert_with(|| value.clone());
            if let Some(object) = self.objects.get(hash) {
                result
                    .objects
                    .entry(*hash)
                    .or_insert_with(|| object.clone());
            }
        } else if let Some(native) = self.natives.get(hash) {
            // A native (extern fn) object a function depends on: also a
            // leaf. Shipping it lets the receiving VM bind the uuid (or
            // fail loudly if its host lacks the implementation).
            result.natives.entry(*hash).or_insert(*native);
            result.providers.entry(*hash).or_insert(*hash);
            if let Some(object) = self.objects.get(hash) {
                result
                    .objects
                    .entry(*hash)
                    .or_insert_with(|| object.clone());
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
/// Layout (integers little-endian; a "string" is `len u32 | utf8`):
///
/// ```text
/// "ABPK" | version u8
/// | has_entry u8 (0|1) | entry hash [32] (if has_entry)
/// | name_count u32 | names: (hash [32] | name string)*
/// | signature_count u32 | signatures: (name string | signature string)*
/// | migration_count u32 | migrations: (cell string | old string | new string)*
/// | object_count u32 | objects: (len u32 | object bytes)*
/// ```
///
/// A wire pack (function shipping) carries no entry or names; an artifact
/// pack carries all of it so the program is runnable by name *and*
/// deployable: the signatures are the rebinding rule's input (a deploy
/// generation's `Binding.signature`), and the migrations are the
/// statically-named `State::init_versioned` obligations pre-swap
/// validation checks (see `ref/live-upgrade.md`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pack {
    /// The program entry point, if this pack is a runnable artifact.
    pub entry_point: Option<blake3::Hash>,
    /// Name → function-hash bindings.
    pub names: Vec<(String, blake3::Hash)>,
    /// Canonical type signature per named item (a subset of `names` —
    /// producers that never rendered one omit the entry).
    pub signatures: Vec<(String, String)>,
    /// Statically-named `State::init_versioned` obligations the pack's
    /// build declared.
    pub migrations: Vec<crate::compiler::MigrationRecord>,
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
            write_string(&mut out, name);
        }
        out.extend_from_slice(&(self.signatures.len() as u32).to_le_bytes());
        for (name, signature) in &self.signatures {
            write_string(&mut out, name);
            write_string(&mut out, signature);
        }
        out.extend_from_slice(&(self.migrations.len() as u32).to_le_bytes());
        for migration in &self.migrations {
            write_string(&mut out, &migration.cell);
            write_string(&mut out, &migration.old);
            write_string(&mut out, &migration.new);
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
                )));
            }
        };

        let name_count = r.u32()? as usize;
        let mut names = Vec::with_capacity(name_count.min(r.remaining()));
        for _ in 0..name_count {
            let hash = r.hash()?;
            let name = r.string()?;
            names.push((name, hash));
        }

        let signature_count = r.u32()? as usize;
        let mut signatures = Vec::with_capacity(signature_count.min(r.remaining()));
        for _ in 0..signature_count {
            let name = r.string()?;
            let signature = r.string()?;
            signatures.push((name, signature));
        }

        let migration_count = r.u32()? as usize;
        let mut migrations = Vec::with_capacity(migration_count.min(r.remaining()));
        for _ in 0..migration_count {
            let cell = r.string()?;
            let old = r.string()?;
            let new = r.string()?;
            migrations.push(crate::compiler::MigrationRecord {
                cell: Arc::from(cell),
                old: Arc::from(old),
                new: Arc::from(new),
            });
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
            signatures,
            migrations,
            objects,
        })
    }
}

/// Append a length-prefixed UTF-8 string to a pack encoding.
fn write_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
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

    fn string(&mut self) -> Result<String, StoreError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        std::str::from_utf8(raw)
            .map(str::to_string)
            .map_err(|_| StoreError::Deserialization("string is not UTF-8".to_string()))
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
mod tests;
