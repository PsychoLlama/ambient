//! The compiled-module output container and its pack (de)serialization.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::CompiledFunction;

/// A compiled module containing all functions ready for execution.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// All compiled functions, keyed by their content-addressed hash.
    pub functions: HashMap<blake3::Hash, CompiledFunction>,

    /// Map from function names to their hashes.
    /// Does NOT include lambdas - they have no names.
    pub function_names: HashMap<Arc<str>, blake3::Hash>,

    /// Map from `const` names to their value-object hashes.
    ///
    /// Only local consts (an imported const is named in its own module).
    /// The hash addresses a [`StoredObject::Value`](crate::object::StoredObject::Value);
    /// these bind names the same way `function_names` do, so a const is a
    /// first-class named binding in the store's `names` index.
    pub const_names: HashMap<Arc<str>, blake3::Hash>,

    /// Map from lambda hashes to their parent function names.
    /// Used for navigation: to find a lambda's source location,
    /// compile the parent and match by hash.
    pub lambda_parents: HashMap<blake3::Hash, Arc<str>>,

    /// The entry point function (typically "run").
    pub entry_point: Option<blake3::Hash>,

    /// Canonical storage objects, keyed by object hash.
    ///
    /// Every function in `functions` is materialized from exactly one of
    /// these objects; recursive groups are stored as a single group object
    /// plus redirect stubs at each member hash. These are the bytes whose
    /// blake3 hash *is* the function identity — persist or transmit these,
    /// not the runtime `functions`.
    pub objects: HashMap<blake3::Hash, crate::object::StoredObject>,
}

impl CompiledModule {
    /// Create an empty compiled module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            function_names: HashMap::new(),
            const_names: HashMap::new(),
            lambda_parents: HashMap::new(),
            entry_point: None,
            objects: HashMap::new(),
        }
    }

    /// Get a function by name.
    #[must_use]
    pub fn get_function(&self, name: &str) -> Option<&CompiledFunction> {
        self.function_names
            .get(name)
            .and_then(|hash| self.functions.get(hash))
    }

    /// Get a function by hash.
    #[must_use]
    pub fn get_function_by_hash(&self, hash: &blake3::Hash) -> Option<&CompiledFunction> {
        self.functions.get(hash)
    }

    /// Merge another compiled module into this one.
    ///
    /// All functions from `other` are added to this module. If there are
    /// hash collisions (same function compiled identically), the existing
    /// function is kept. Name collisions are handled by keeping the first
    /// occurrence.
    pub fn merge(&mut self, other: &CompiledModule) {
        for (hash, func) in &other.functions {
            self.functions.entry(*hash).or_insert_with(|| func.clone());
        }
        for (name, hash) in &other.function_names {
            self.function_names.entry(Arc::clone(name)).or_insert(*hash);
        }
        for (name, hash) in &other.const_names {
            self.const_names.entry(Arc::clone(name)).or_insert(*hash);
        }
        for (hash, parent) in &other.lambda_parents {
            self.lambda_parents
                .entry(*hash)
                .or_insert_with(|| Arc::clone(parent));
        }
        for (hash, object) in &other.objects {
            self.objects.entry(*hash).or_insert_with(|| object.clone());
        }
        // Don't overwrite entry point if we already have one
        if self.entry_point.is_none() {
            self.entry_point = other.entry_point;
        }
    }

    /// Package this module as a runnable artifact pack: every canonical
    /// object plus the name bindings and entry point.
    #[must_use]
    pub fn to_pack(&self) -> crate::store::Pack {
        // Functions and consts share one flat name index; the object kind at
        // each hash (function vs `Value`) distinguishes them on the far side.
        let mut names: Vec<(String, blake3::Hash)> = self
            .function_names
            .iter()
            .chain(self.const_names.iter())
            .map(|(name, hash)| (name.to_string(), *hash))
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));

        // Redirects are derived from groups, so packs never carry them.
        let mut object_hashes: Vec<&blake3::Hash> = self
            .objects
            .iter()
            .filter(|(_, o)| !matches!(o, crate::object::StoredObject::Redirect { .. }))
            .map(|(h, _)| h)
            .collect();
        object_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

        crate::store::Pack {
            entry_point: self.entry_point,
            names,
            objects: object_hashes
                .iter()
                .map(|h| self.objects[*h].clone())
                .collect(),
        }
    }

    /// Reconstruct a runnable module from an artifact pack.
    ///
    /// Every function is materialized from its canonical object, so all
    /// hashes are recomputed from content — a tampered pack cannot smuggle
    /// code under a false hash.
    ///
    /// # Errors
    ///
    /// Returns an error if an object is malformed.
    pub fn from_pack(pack: &crate::store::Pack) -> Result<Self, crate::store::StoreError> {
        let mut module = Self::new();
        module.entry_point = pack.entry_point;

        for object in &pack.objects {
            if matches!(object, crate::object::StoredObject::Redirect { .. }) {
                // Legacy safety: packs shouldn't carry redirects, and one
                // without its group is meaningless. Regenerated below.
                continue;
            }
            let object_hash = object.hash();
            let materialized = object
                .materialize()
                .map_err(crate::store::StoreError::Object)?;
            let is_group =
                matches!(object, crate::object::StoredObject::Group(members) if members.len() > 1);
            for (index, (hash, func)) in materialized.into_iter().enumerate() {
                if is_group {
                    // Re-derive the redirect stubs a disk store needs to
                    // resolve member hashes back to their group.
                    module.objects.insert(
                        hash,
                        crate::object::StoredObject::Redirect {
                            group: object_hash,
                            index: index as u32,
                        },
                    );
                }
                module.functions.insert(hash, func);
            }
            module.objects.insert(object_hash, object.clone());
        }

        // Route each name to the right index by the kind of object it binds:
        // a `Value` object is a const, everything else a function.
        for (name, hash) in &pack.names {
            let is_const = matches!(
                module.objects.get(hash),
                Some(crate::object::StoredObject::Value(_))
            );
            let table = if is_const {
                &mut module.const_names
            } else {
                &mut module.function_names
            };
            table.insert(Arc::from(name.as_str()), *hash);
        }

        Ok(module)
    }
}

impl Default for CompiledModule {
    fn default() -> Self {
        Self::new()
    }
}
