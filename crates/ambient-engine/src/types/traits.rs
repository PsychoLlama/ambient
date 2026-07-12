//! The trait system: definitions, implementations, coherence, and the
//! impl-method dispatch symbols.
//!
//! Traits are **nominal**, exactly like enums, structs, and abilities: the
//! mandatory `unique(<uuid>)` prefix is the trait's identity. Every table
//! here keys off that uuid — never the name — so renaming a trait never
//! changes what a bound or an impl means, and two same-shaped traits never
//! unify. Names exist for display and for in-scope lookup only.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use super::{NominalType, Type};

// ─────────────────────────────────────────────────────────────────────────────
// Reserved trait identities
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical identity of the prelude `Add` trait (`core::traits::Add`).
///
/// The operator traits are ordinary declarations in `core::traits`, but the
/// engine's operator desugar (`a + b` → `a.add(b)`) must name *the* trait an
/// operator dispatches through, independent of what is lexically in scope —
/// a user trait named `Add` must never capture `+`. These reserved uuids are
/// that anchor, in the same `0xffff…` namespace as `Option`/`Result`
/// ([`super::OPTION_UUID`]); the source declarations in `core_lib/traits.ab`
/// claim them and are pinned by `validate_reserved_trait`, so the sources
/// and the engine can never drift and no other module can hijack one.
///
/// Discriminators `0x0010`–`0x001f` are reserved for traits; see
/// [`super::BOOL_UUID`] for how this namespace is allocated.
pub const TRAIT_ADD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0010);
/// Canonical identity of the prelude `Sub` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_SUB_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0011);
/// Canonical identity of the prelude `Mul` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_MUL_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0012);
/// Canonical identity of the prelude `Div` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_DIV_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0013);
/// Canonical identity of the prelude `Mod` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_MOD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0014);
/// Canonical identity of the prelude `Eq` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_EQ_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0015);
/// Canonical identity of the prelude `Ord` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_ORD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0016);
/// Canonical identity of the `core::traits::Default` trait (not in the
/// prelude — no operator desugars to it). See [`TRAIT_ADD_UUID`].
pub const TRAIT_DEFAULT_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0017);

/// A reserved core trait: name/uuid pairs for the declarations in
/// `core_lib/traits.ab`, the trait analogue of [`super::Primitive`] /
/// [`super::Container`]. `validate_reserved_trait` pins a declaration
/// claiming either half to the canonical pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedTrait {
    /// `Add` — the `+` operator.
    Add,
    /// `Sub` — the `-` operator.
    Sub,
    /// `Mul` — the `*` operator.
    Mul,
    /// `Div` — the `/` operator.
    Div,
    /// `Mod` — the `%` operator.
    Mod,
    /// `Eq` — the `==`/`!=` operators.
    Eq,
    /// `Ord` — the `<`/`<=`/`>`/`>=` operators.
    Ord,
    /// `Default` — no operator; standard-library convenience.
    Default,
}

impl ReservedTrait {
    /// Every reserved core trait.
    pub const ALL: [Self; 8] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Mod,
        Self::Eq,
        Self::Ord,
        Self::Default,
    ];

    /// The reserved identity uuid for this trait.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        match self {
            Self::Add => TRAIT_ADD_UUID,
            Self::Sub => TRAIT_SUB_UUID,
            Self::Mul => TRAIT_MUL_UUID,
            Self::Div => TRAIT_DIV_UUID,
            Self::Mod => TRAIT_MOD_UUID,
            Self::Eq => TRAIT_EQ_UUID,
            Self::Ord => TRAIT_ORD_UUID,
            Self::Default => TRAIT_DEFAULT_UUID,
        }
    }

    /// The canonical trait name, as spelled in `core_lib/traits.ab`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::Mul => "Mul",
            Self::Div => "Div",
            Self::Mod => "Mod",
            Self::Eq => "Eq",
            Self::Ord => "Ord",
            Self::Default => "Default",
        }
    }

    /// The reserved trait matching a uuid, if any.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.uuid() == uuid)
    }

    /// The reserved trait matching a canonical name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait System Types
// ─────────────────────────────────────────────────────────────────────────────

/// A resolved trait bound (`T: Eq`): the trait's identity plus its spelled
/// name for diagnostics. Everything semantic keys off the uuid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitBound {
    /// The bound trait's identity.
    pub trait_uuid: Uuid,
    /// The name the bound was written as (display only).
    pub name: Arc<str>,
}

/// Definition of a trait.
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// The trait's nominal identity (its `unique(<uuid>)` prefix).
    pub uuid: Uuid,

    /// Trait name for display and in-scope lookup.
    pub name: Arc<str>,

    /// Methods defined by this trait, in declaration order.
    pub methods: Vec<TraitMethodDef>,
}

impl TraitDef {
    /// Create a new trait definition.
    #[must_use]
    pub fn new(uuid: Uuid, name: impl Into<Arc<str>>) -> Self {
        Self {
            uuid,
            name: name.into(),
            methods: Vec::new(),
        }
    }

    /// Add a method.
    #[must_use]
    pub fn with_method(mut self, method: TraitMethodDef) -> Self {
        self.methods.push(method);
        self
    }

    /// The dictionary slot order for this trait: method indices sorted by
    /// method name. A bounded generic function compiles bound-method calls
    /// as tuple accesses into a dictionary argument, and call sites build
    /// that tuple from a concrete impl — both sides derive the layout from
    /// this one function, so they can never disagree within a build.
    /// (Cross-build agreement is the content-addressing story: dictionary
    /// construction and slot access are both *bytecode*, covered by the
    /// caller's and callee's hashes.)
    #[must_use]
    pub fn dictionary_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.methods.len()).collect();
        order.sort_by(|&a, &b| self.methods[a].name.cmp(&self.methods[b].name));
        order
    }

    /// The dictionary slot index of a method, by name. See
    /// [`dictionary_order`](Self::dictionary_order).
    #[must_use]
    pub fn dictionary_slot(&self, method_name: &str) -> Option<usize> {
        self.dictionary_order()
            .into_iter()
            .position(|idx| self.methods[idx].name.as_ref() == method_name)
    }
}

/// A method signature in a trait definition.
///
/// The signature is stored *un-instantiated*: parameter/return types keep
/// `Self` and any method-level type parameters (`U`) as bare `Named`s, and the
/// effect row keeps `E` unresolved. `type_param_names`/`ability_var_names`
/// record the method's own generics so each use site (a trait-dispatched call,
/// or an impl body) resolves the signature under its own scope — fresh
/// inference variables at a call site, rigid parameters plus a fresh row scope
/// in an impl body — instead of baking one set of variables into the registry.
#[derive(Debug, Clone)]
pub struct TraitMethodDef {
    /// Method name.
    pub name: Arc<str>,

    /// Whether the method takes `self` as first argument.
    pub has_self: bool,

    /// Parameter types (excluding self), with `Self`/`U`/`E` unresolved.
    pub params: Vec<Type>,

    /// Return type, with `Self`/`U`/`E` unresolved.
    pub ret: Type,

    /// The method's declared effect row (`with E` / `with Stdio`), unresolved.
    /// A name matching `ability_var_names` is the row's polymorphic tail.
    pub abilities: Vec<crate::ast::QualifiedName>,

    /// Unbounded method-level type parameter names (`fn tag<U>` → `["U"]`).
    pub type_param_names: Vec<Arc<str>>,

    /// Method-level ability (row) variable names (`fn each<E!>` → `["E"]`).
    pub ability_var_names: Vec<Arc<str>>,
}

impl TraitMethodDef {
    /// Create a new trait method definition with no method-level generics.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, has_self: bool, params: Vec<Type>, ret: Type) -> Self {
        Self {
            name: name.into(),
            has_self,
            params,
            ret,
            abilities: Vec::new(),
            type_param_names: Vec::new(),
            ability_var_names: Vec::new(),
        }
    }

    /// Attach the method's declared effect row and method-level generics.
    #[must_use]
    pub fn with_generics(
        mut self,
        abilities: Vec<crate::ast::QualifiedName>,
        type_param_names: Vec<Arc<str>>,
        ability_var_names: Vec<Arc<str>>,
    ) -> Self {
        self.abilities = abilities;
        self.type_param_names = type_param_names;
        self.ability_var_names = ability_var_names;
        self
    }
}

/// A registered trait implementation.
#[derive(Debug, Clone)]
pub struct TraitImpl {
    /// The identity of the trait being implemented.
    pub trait_uuid: Uuid,

    /// The type implementing the trait (must be nominal).
    pub implementing_type: NominalType,

    /// Whether the impl block declares its own type parameters
    /// (`impl<T> Show for Wrapper<T>`). A generic impl cannot yet serve as
    /// a dictionary source — building its dictionary would mean closing
    /// each method over the dictionaries *its* bounds demand — so bound
    /// solving reports it unsupported instead of miscompiling.
    pub is_generic: bool,

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
    pub fn new(trait_uuid: Uuid, implementing_type: NominalType) -> Self {
        Self {
            trait_uuid,
            implementing_type,
            is_generic: false,
            methods: HashMap::new(),
        }
    }

    /// Mark this impl as generic (declared with its own type parameters).
    #[must_use]
    pub fn with_generic(mut self, is_generic: bool) -> Self {
        self.is_generic = is_generic;
        self
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
/// other function. The symbol is derived only from source-stable identities —
/// the implementing type's UUID, the trait's UUID, and the method name —
/// never from names that can collide (two same-named traits implemented for
/// one type must not share a symbol) or from compilation-order artifacts.
/// The `::` separator cannot appear in module-qualified names (which use
/// `.`), so these symbols never collide with user functions.
#[must_use]
pub fn impl_method_symbol(type_uuid: &Uuid, trait_uuid: &Uuid, method_name: &str) -> Arc<str> {
    format!(
        "{}::{}::{method_name}",
        uuid_to_source(type_uuid),
        uuid_to_source(trait_uuid)
    )
    .into()
}

/// Render a UUID in Ambient's canonical source form: uppercase, hyphenated.
///
/// UUID literals are written uppercase in source; the `uuid` crate otherwise
/// renders them lowercase. Anywhere a UUID is shown to a user or embedded in a
/// symbol name — `unique(...)` type display and the `<uuid>::method` /
/// `<type-uuid>::<trait-uuid>::<method>` symbols — goes through this so the
/// rendered form matches the source syntax and round-trips.
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
        /// The identity of the trait providing the method.
        trait_uuid: Uuid,
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
///
/// Definitions are keyed by the trait's identity uuid; `name_to_uuid` is the
/// in-scope lookup table (imports and locals register here, later
/// registrations shadowing earlier ones — the same precedence every other
/// name follows). Impls key on `(trait uuid, type uuid)`, the coherence
/// granularity.
#[derive(Debug, Clone, Default)]
pub struct TraitRegistry {
    /// Map from trait identity to trait definition.
    traits: HashMap<Uuid, TraitDef>,

    /// Map from in-scope trait name to identity.
    name_to_uuid: HashMap<Arc<str>, Uuid>,

    /// Every known trait per name, in-scope or not — the disambiguation
    /// fallback for names that reach this check from *another* module's
    /// signature (a foreign `fn f<T: Show>` hydrated here spells `Show` in
    /// the defining module's scope, which this module may not share).
    all_names: HashMap<Arc<str>, Vec<Uuid>>,

    /// Map from (trait uuid, nominal type UUID) to implementation.
    impls: HashMap<(Uuid, Uuid), TraitImpl>,
}

impl TraitRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a trait definition, binding its name in scope.
    pub fn register_trait(&mut self, def: TraitDef) {
        self.name_to_uuid.insert(def.name.clone(), def.uuid);
        self.index_name(&def);
        self.traits.insert(def.uuid, def);
    }

    /// Register a trait definition *without* binding its bare name in
    /// scope — the identity resolves ([`get_trait`](Self::get_trait)) and
    /// the name is indexed for the [`lookup_trait`](Self::lookup_trait)
    /// fallback, but a local/imported trait of the same name still wins.
    /// Used for foreign traits a module never imported: their impls and
    /// the bounds of foreign signatures must still resolve.
    pub fn register_trait_unnamed(&mut self, def: TraitDef) {
        self.index_name(&def);
        self.traits.entry(def.uuid).or_insert(def);
    }

    fn index_name(&mut self, def: &TraitDef) {
        let entry = self.all_names.entry(def.name.clone()).or_default();
        if !entry.contains(&def.uuid) {
            entry.push(def.uuid);
        }
    }

    /// Get trait definition by identity.
    #[must_use]
    pub fn get_trait(&self, uuid: Uuid) -> Option<&TraitDef> {
        self.traits.get(&uuid)
    }

    /// Resolve an *in-scope* trait name to its identity: locals and
    /// imports, later registrations shadowing earlier. This is the lookup
    /// for everything spelled in the current module — impl headers, local
    /// bounds — so trait definitions stay import-scoped (`Default` is
    /// unavailable without `use core::traits::Default`).
    #[must_use]
    pub fn lookup_trait(&self, name: &str) -> Option<Uuid> {
        self.name_to_uuid.get(name).copied()
    }

    /// Resolve a trait name arriving through a *foreign* signature (`fn
    /// f<T: Show>` hydrated from another module): the in-scope binding
    /// first, then the build-global index when unambiguous. The name was
    /// spelled in the defining module's scope, which this module need not
    /// share, so scope alone would spuriously reject valid imports of
    /// bounded functions. Two same-named traits in one build resolve only
    /// through the in-scope binding.
    #[must_use]
    pub fn lookup_trait_lenient(&self, name: &str) -> Option<Uuid> {
        if let Some(uuid) = self.name_to_uuid.get(name) {
            return Some(*uuid);
        }
        match self.all_names.get(name).map(Vec::as_slice) {
            Some([unique]) => Some(*unique),
            _ => None,
        }
    }

    /// Register a trait implementation.
    ///
    /// Returns the previously registered impl for the same
    /// `(trait, type)` pair, if any — a coherence violation the caller
    /// should report.
    pub fn register_impl(&mut self, impl_: TraitImpl) -> Option<TraitImpl> {
        let key = (impl_.trait_uuid, impl_.implementing_type.uuid);
        self.impls.insert(key, impl_)
    }

    /// Get implementation for a trait and nominal type.
    #[must_use]
    pub fn get_impl(&self, trait_uuid: Uuid, type_uuid: Uuid) -> Option<&TraitImpl> {
        self.impls.get(&(trait_uuid, type_uuid))
    }

    /// Find all implementations for a nominal type.
    ///
    /// Sorted by trait uuid so lookups are deterministic (the backing map
    /// has arbitrary iteration order).
    #[must_use]
    pub fn impls_for_type(&self, type_uuid: Uuid) -> Vec<&TraitImpl> {
        let mut impls: Vec<&TraitImpl> = self
            .impls
            .iter()
            .filter(|((_, uuid), _)| *uuid == type_uuid)
            .map(|(_, impl_)| impl_)
            .collect();
        impls.sort_by_key(|impl_| impl_.trait_uuid);
        impls
    }

    /// Find a method by name for a given nominal type.
    #[must_use]
    pub fn find_method(&self, type_uuid: Uuid, method_name: &str) -> MethodLookup<'_> {
        let mut matches: Vec<(Uuid, &TraitMethodDef, Arc<str>)> = Vec::new();
        for impl_ in self.impls_for_type(type_uuid) {
            if let Some(symbol) = impl_.methods.get(method_name)
                && let Some(trait_def) = self.get_trait(impl_.trait_uuid)
                && let Some(method) = trait_def
                    .methods
                    .iter()
                    .find(|m| m.name.as_ref() == method_name)
            {
                matches.push((impl_.trait_uuid, method, Arc::clone(symbol)));
            }
        }

        match matches.len() {
            0 => MethodLookup::NotFound,
            1 => {
                // Vec::swap_remove on a single-element vec cannot fail.
                let (trait_uuid, method, symbol) = matches.swap_remove(0);
                MethodLookup::Found {
                    trait_uuid,
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
    pub fn implements(&self, type_uuid: Uuid, trait_uuid: Uuid) -> bool {
        self.impls.contains_key(&(trait_uuid, type_uuid))
    }
}
