//! The trait system: definitions, implementations, coherence, and the
//! impl-method dispatch symbols.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use super::{NominalType, TraitId, Type, TypeVarId};

// ─────────────────────────────────────────────────────────────────────────────
// Trait System Types
// ─────────────────────────────────────────────────────────────────────────────

/// Definition of a trait.
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// Unique trait identifier.
    pub id: TraitId,

    /// Trait name for display purposes.
    pub name: Arc<str>,

    /// Type parameters for generic traits.
    pub type_params: Vec<TypeVarId>,

    /// Methods defined by this trait.
    pub methods: Vec<TraitMethodDef>,

    /// Supertraits that must also be implemented.
    pub supertraits: Vec<TraitId>,
}

impl TraitDef {
    /// Create a new trait definition.
    #[must_use]
    pub fn new(id: TraitId, name: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            name: name.into(),
            type_params: Vec::new(),
            methods: Vec::new(),
            supertraits: Vec::new(),
        }
    }

    /// Add a type parameter.
    #[must_use]
    pub fn with_type_param(mut self, var: TypeVarId) -> Self {
        self.type_params.push(var);
        self
    }

    /// Add a method.
    #[must_use]
    pub fn with_method(mut self, method: TraitMethodDef) -> Self {
        self.methods.push(method);
        self
    }

    /// Add a supertrait.
    #[must_use]
    pub fn with_supertrait(mut self, trait_id: TraitId) -> Self {
        self.supertraits.push(trait_id);
        self
    }
}

/// A method signature in a trait definition.
#[derive(Debug, Clone)]
pub struct TraitMethodDef {
    /// Method name.
    pub name: Arc<str>,

    /// Whether the method takes `self` as first argument.
    pub has_self: bool,

    /// Parameter types (excluding self).
    pub params: Vec<Type>,

    /// Return type.
    pub ret: Type,
}

impl TraitMethodDef {
    /// Create a new trait method definition.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, has_self: bool, params: Vec<Type>, ret: Type) -> Self {
        Self {
            name: name.into(),
            has_self,
            params,
            ret,
        }
    }
}

/// A registered trait implementation.
#[derive(Debug, Clone)]
pub struct TraitImpl {
    /// The trait being implemented.
    pub trait_id: TraitId,

    /// The type implementing the trait (must be nominal).
    pub implementing_type: NominalType,

    /// Method dispatch symbols: method name -> canonical impl-method symbol.
    ///
    /// The symbol (see [`impl_method_symbol`]) names the compiled method
    /// function; the compiler resolves it to a content-addressed hash the
    /// same way it resolves ordinary function names.
    pub methods: HashMap<Arc<str>, Arc<str>>,
}

impl TraitImpl {
    /// Create a new trait implementation.
    #[must_use]
    pub fn new(trait_id: TraitId, implementing_type: NominalType) -> Self {
        Self {
            trait_id,
            implementing_type,
            methods: HashMap::new(),
        }
    }

    /// Add a method implementation.
    #[must_use]
    pub fn with_method(mut self, name: impl Into<Arc<str>>, symbol: impl Into<Arc<str>>) -> Self {
        self.methods.insert(name.into(), symbol.into());
        self
    }
}

/// The canonical function symbol for an impl method.
///
/// Impl methods are compiled as ordinary named functions under this symbol,
/// so they flow through the same content-addressed hash finalization as any
/// other function. The symbol is derived only from source-stable data (the
/// nominal type's UUID and source-level names) — never from compilation-order
/// artifacts like trait IDs — so it is deterministic across compilation
/// contexts. The `::` separator cannot appear in module-qualified names
/// (which use `.`), so these symbols never collide with user functions.
#[must_use]
pub fn impl_method_symbol(type_uuid: &Uuid, trait_name: &str, method_name: &str) -> Arc<str> {
    format!("{}::{trait_name}::{method_name}", uuid_to_source(type_uuid)).into()
}

/// Render a UUID in Ambient's canonical source form: uppercase, hyphenated.
///
/// UUID literals are written uppercase in source; the `uuid` crate otherwise
/// renders them lowercase. Anywhere a UUID is shown to a user or embedded in a
/// symbol name — `unique(...)` type display and the `<uuid>::method` /
/// `<uuid>::<Trait>::<method>` symbols — goes through this so the rendered form
/// matches the source syntax and round-trips.
#[must_use]
pub fn uuid_to_source(uuid: &Uuid) -> String {
    uuid.hyphenated().to_string().to_uppercase()
}

/// Result of looking up a method by name on a nominal type.
#[derive(Debug)]
pub enum MethodLookup<'a> {
    /// No trait implemented for the type provides the method.
    NotFound,
    /// Exactly one implementation provides the method.
    Found {
        /// The trait providing the method.
        trait_id: TraitId,
        /// The trait's method signature.
        method: &'a TraitMethodDef,
        /// The canonical dispatch symbol (see [`impl_method_symbol`]).
        symbol: Arc<str>,
    },
    /// Multiple traits implemented for the type provide a method with this
    /// name; the call must be disambiguated.
    Ambiguous {
        /// Names of the traits that provide the method.
        traits: Vec<Arc<str>>,
    },
}

/// Registry of trait definitions and implementations.
#[derive(Debug, Clone, Default)]
pub struct TraitRegistry {
    /// Map from trait ID to trait definition.
    traits: HashMap<TraitId, TraitDef>,

    /// Map from trait name to ID for lookup.
    name_to_id: HashMap<Arc<str>, TraitId>,

    /// Map from (trait ID, nominal type UUID) to implementation.
    impls: HashMap<(TraitId, Uuid), TraitImpl>,

    /// Next available trait ID.
    next_id: TraitId,
}

impl TraitRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Generate a fresh trait ID.
    pub fn fresh_id(&mut self) -> TraitId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Register a trait definition.
    pub fn register_trait(&mut self, def: TraitDef) {
        self.name_to_id.insert(def.name.clone(), def.id);
        self.traits.insert(def.id, def);
    }

    /// Get trait definition by ID.
    #[must_use]
    pub fn get_trait(&self, id: TraitId) -> Option<&TraitDef> {
        self.traits.get(&id)
    }

    /// Look up trait ID by name.
    #[must_use]
    pub fn lookup_trait(&self, name: &str) -> Option<TraitId> {
        self.name_to_id.get(name).copied()
    }

    /// Register a trait implementation.
    ///
    /// Returns the previously registered impl for the same
    /// `(trait, type)` pair, if any — a coherence violation the caller
    /// should report.
    pub fn register_impl(&mut self, impl_: TraitImpl) -> Option<TraitImpl> {
        let key = (impl_.trait_id, impl_.implementing_type.uuid);
        self.impls.insert(key, impl_)
    }

    /// Get implementation for a trait and nominal type.
    #[must_use]
    pub fn get_impl(&self, trait_id: TraitId, type_uuid: Uuid) -> Option<&TraitImpl> {
        self.impls.get(&(trait_id, type_uuid))
    }

    /// Find all implementations for a nominal type.
    ///
    /// Sorted by trait ID so lookups are deterministic (the backing map has
    /// arbitrary iteration order).
    #[must_use]
    pub fn impls_for_type(&self, type_uuid: Uuid) -> Vec<&TraitImpl> {
        let mut impls: Vec<&TraitImpl> = self
            .impls
            .iter()
            .filter(|((_, uuid), _)| *uuid == type_uuid)
            .map(|(_, impl_)| impl_)
            .collect();
        impls.sort_by_key(|impl_| impl_.trait_id);
        impls
    }

    /// Find a method by name for a given nominal type.
    #[must_use]
    pub fn find_method(&self, type_uuid: Uuid, method_name: &str) -> MethodLookup<'_> {
        let mut matches: Vec<(TraitId, &TraitMethodDef, Arc<str>)> = Vec::new();
        for impl_ in self.impls_for_type(type_uuid) {
            if let Some(symbol) = impl_.methods.get(method_name)
                && let Some(trait_def) = self.get_trait(impl_.trait_id)
                && let Some(method) = trait_def
                    .methods
                    .iter()
                    .find(|m| m.name.as_ref() == method_name)
            {
                matches.push((impl_.trait_id, method, Arc::clone(symbol)));
            }
        }

        match matches.len() {
            0 => MethodLookup::NotFound,
            1 => {
                // Vec::swap_remove on a single-element vec cannot fail.
                let (trait_id, method, symbol) = matches.swap_remove(0);
                MethodLookup::Found {
                    trait_id,
                    method,
                    symbol,
                }
            }
            _ => MethodLookup::Ambiguous {
                traits: matches
                    .iter()
                    .filter_map(|(id, _, _)| self.get_trait(*id).map(|t| Arc::clone(&t.name)))
                    .collect(),
            },
        }
    }

    /// Check if a type implements a trait.
    #[must_use]
    pub fn implements(&self, type_uuid: Uuid, trait_id: TraitId) -> bool {
        self.impls.contains_key(&(trait_id, type_uuid))
    }
}
