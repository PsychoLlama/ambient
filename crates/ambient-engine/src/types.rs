//! Type system for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with support for:
//! - Primitive types (number, string, bool, unit)
//! - Composite types (tuples, records, functions)
//! - Polymorphic types (generics with type variables)
//! - Nominal types (unique types distinguished by UUID)
//! - Ability types for tracking side effects (Milestone 8)
//!
//! The type system uses structural equivalence by default, with nominal
//! types providing opt-in name-based distinction.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use uuid::Uuid;

/// A unique identifier for type variables, used during unification.
pub type TypeVarId = u32;

/// A unique identifier for ability variables, used during ability inference.
pub type AbilityVarId = u32;

/// An ability identifier: the content-addressed identity of the ability's
/// canonical interface (re-exported from `ambient-core`).
pub use ambient_core::AbilityId;

/// A unique identifier for traits.
pub type TraitId = u16;

/// Counter for generating fresh type variable IDs.
#[derive(Debug, Default)]
pub struct TypeVarGen {
    next_id: TypeVarId,
    next_ability_id: AbilityVarId,
}

impl TypeVarGen {
    /// Create a new type variable generator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 0,
            next_ability_id: 0,
        }
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        let id = self.next_id;
        self.next_id += 1;
        Type::Var(TypeVar::Unbound(id))
    }

    /// Generate a fresh type variable ID.
    pub fn fresh_id(&mut self) -> TypeVarId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Generate a fresh ability variable.
    pub fn fresh_ability_var(&mut self) -> AbilitySet {
        let id = self.next_ability_id;
        self.next_ability_id += 1;
        AbilitySet::Var(id)
    }

    /// Generate a fresh ability variable ID.
    pub fn fresh_ability_id(&mut self) -> AbilityVarId {
        let id = self.next_ability_id;
        self.next_ability_id += 1;
        id
    }
}

/// A type variable that may be unbound or linked to another type.
///
/// Type variables are used during inference to represent unknown types.
/// During unification, they get linked to concrete types or other variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeVar {
    /// An unbound type variable with a unique ID.
    Unbound(TypeVarId),

    /// A type variable that has been linked to another type.
    /// Uses interior mutability for efficient union-find during unification.
    Link(Rc<RefCell<Type>>),
}

// ─────────────────────────────────────────────────────────────────────────────
// Ability Types (Milestone 8)
// ─────────────────────────────────────────────────────────────────────────────

/// A set of abilities required or provided by a function.
///
/// Abilities can be:
/// - Empty: A pure function with no effects
/// - Concrete: A specific set of ability IDs
/// - Variable: A polymorphic ability variable (for `E!` syntax)
/// - Row: A concrete set plus a polymorphic tail (for `Filesystem, E!` syntax)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AbilitySet {
    /// Empty set of abilities (pure function).
    #[default]
    Empty,

    /// A concrete set of ability IDs.
    Concrete(Vec<AbilityId>),

    /// An ability variable (for polymorphic effects like `E!`).
    Var(AbilityVarId),

    /// A row: concrete abilities plus a variable tail.
    /// Represents `{Ability1, Ability2, ...} ∪ E!`
    Row {
        /// Known concrete abilities.
        concrete: Vec<AbilityId>,
        /// Polymorphic tail variable.
        tail: AbilityVarId,
    },

    /// Ability names from source annotations that have not been resolved to
    /// IDs yet (e.g. `(T) -> U with Console`). Produced by lowering, which
    /// has no ability resolver; eliminated by `Infer::resolve_holes` before
    /// any unification. Must never survive type checking.
    Unresolved(Vec<Arc<str>>),
}

impl AbilitySet {
    /// Create an empty ability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self::Empty
    }

    /// Create a concrete ability set from a single ability.
    #[must_use]
    pub fn single(ability: AbilityId) -> Self {
        Self::Concrete(vec![ability])
    }

    /// Create a concrete ability set from multiple abilities.
    #[must_use]
    pub fn from_abilities(abilities: impl IntoIterator<Item = AbilityId>) -> Self {
        let mut abilities: Vec<_> = abilities.into_iter().collect();
        if abilities.is_empty() {
            Self::Empty
        } else {
            abilities.sort_unstable();
            abilities.dedup();
            Self::Concrete(abilities)
        }
    }

    /// Create an ability variable.
    #[must_use]
    pub const fn var(id: AbilityVarId) -> Self {
        Self::Var(id)
    }

    /// Create a row with concrete abilities and a variable tail.
    #[must_use]
    pub fn row(abilities: impl IntoIterator<Item = AbilityId>, tail: AbilityVarId) -> Self {
        let mut concrete: Vec<_> = abilities.into_iter().collect();
        if concrete.is_empty() {
            Self::Var(tail)
        } else {
            concrete.sort_unstable();
            concrete.dedup();
            Self::Row { concrete, tail }
        }
    }

    /// Check if this is an empty ability set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Check if this is a pure (no effects) ability set.
    #[must_use]
    pub fn is_pure(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Check if this set contains a specific ability.
    #[must_use]
    pub fn contains(&self, ability: AbilityId) -> bool {
        match self {
            // Variable might contain it, but we don't know
            Self::Empty | Self::Var(_) | Self::Unresolved(_) => false,
            Self::Concrete(abilities) => abilities.contains(&ability),
            Self::Row { concrete, .. } => concrete.contains(&ability),
        }
    }

    /// Get all concrete abilities in this set.
    #[must_use]
    pub fn concrete_abilities(&self) -> &[AbilityId] {
        match self {
            Self::Empty | Self::Var(_) | Self::Unresolved(_) => &[],
            Self::Concrete(abilities) => abilities,
            Self::Row { concrete, .. } => concrete,
        }
    }

    /// Get the ability variable if this is a Var or Row.
    #[must_use]
    pub fn ability_var(&self) -> Option<AbilityVarId> {
        match self {
            Self::Var(id) | Self::Row { tail: id, .. } => Some(*id),
            _ => None,
        }
    }

    /// Collect all free ability variables.
    #[must_use]
    pub fn free_ability_vars(&self) -> Vec<AbilityVarId> {
        match self {
            Self::Empty | Self::Concrete(_) | Self::Unresolved(_) => Vec::new(),
            Self::Var(id) | Self::Row { tail: id, .. } => vec![*id],
        }
    }

    /// Union two ability sets.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Empty, other) | (other, Self::Empty) => other.clone(),
            (Self::Concrete(a), Self::Concrete(b)) => {
                let mut combined: Vec<_> = a.iter().chain(b.iter()).copied().collect();
                combined.sort_unstable();
                combined.dedup();
                Self::Concrete(combined)
            }
            (Self::Concrete(concrete), Self::Var(tail))
            | (Self::Var(tail), Self::Concrete(concrete)) => Self::Row {
                concrete: concrete.clone(),
                tail: *tail,
            },
            (Self::Concrete(a), Self::Row { concrete: b, tail })
            | (Self::Row { concrete: b, tail }, Self::Concrete(a)) => {
                let mut combined: Vec<_> = a.iter().chain(b.iter()).copied().collect();
                combined.sort_unstable();
                combined.dedup();
                Self::Row {
                    concrete: combined,
                    tail: *tail,
                }
            }
            // Two variables or two rows can't merge without unification
            // (which handles them later). Unresolved names can't be combined
            // before resolution; resolve_holes eliminates them before unions
            // matter. Return self in both cases.
            (Self::Var(_) | Self::Row { .. }, Self::Var(_) | Self::Row { .. })
            | (Self::Unresolved(_), _)
            | (_, Self::Unresolved(_)) => self.clone(),
        }
    }
}

impl fmt::Display for AbilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "{{}}"),
            Self::Concrete(abilities) => {
                write!(f, "{{")?;
                for (i, ability) in abilities.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{ability}")?;
                }
                write!(f, "}}")
            }
            Self::Var(id) => write!(f, "E{id}!"),
            Self::Row { concrete, tail } => {
                write!(f, "{{")?;
                for (i, ability) in concrete.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{ability}")?;
                }
                write!(f, ", E{tail}!}}")
            }
            Self::Unresolved(names) => {
                write!(f, "{{")?;
                for (i, name) in names.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}?")?;
                }
                write!(f, "}}")
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ability Registry
// ─────────────────────────────────────────────────────────────────────────────

/// Information about an ability definition.
#[derive(Debug, Clone)]
pub struct AbilityInfo {
    /// The ability name.
    pub name: Arc<str>,
    /// Dependencies (other abilities this one requires).
    pub dependencies: Vec<AbilityId>,
    /// Method signatures: name -> (params, return type).
    pub methods: HashMap<Arc<str>, (Vec<Type>, Type)>,
}

impl AbilityInfo {
    /// Create a new ability info.
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            dependencies: Vec::new(),
            methods: HashMap::new(),
        }
    }

    /// Add a dependency.
    #[must_use]
    pub fn with_dependency(mut self, dep: AbilityId) -> Self {
        self.dependencies.push(dep);
        self
    }

    /// Add a method.
    #[must_use]
    pub fn with_method(mut self, name: impl Into<Arc<str>>, params: Vec<Type>, ret: Type) -> Self {
        self.methods.insert(name.into(), (params, ret));
        self
    }
}

/// Registry of ability definitions for dependency tracking.
#[derive(Debug, Clone, Default)]
pub struct AbilityRegistry {
    /// Map from ability ID to ability info.
    abilities: HashMap<AbilityId, AbilityInfo>,
    /// Map from ability name to ID for lookup.
    name_to_id: HashMap<Arc<str>, AbilityId>,
}

impl AbilityRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an ability.
    pub fn register(&mut self, id: AbilityId, info: AbilityInfo) {
        self.name_to_id.insert(info.name.clone(), id);
        self.abilities.insert(id, info);
    }

    /// Get ability info by ID.
    #[must_use]
    pub fn get(&self, id: AbilityId) -> Option<&AbilityInfo> {
        self.abilities.get(&id)
    }

    /// Look up ability ID by name.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<AbilityId> {
        self.name_to_id.get(name).copied()
    }

    /// Get the transitive closure of dependencies for an ability.
    /// Returns all abilities that must be available when this ability is used.
    #[must_use]
    pub fn transitive_dependencies(&self, id: AbilityId) -> Vec<AbilityId> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.collect_dependencies(id, &mut result, &mut visited);
        result
    }

    fn collect_dependencies(
        &self,
        id: AbilityId,
        result: &mut Vec<AbilityId>,
        visited: &mut std::collections::HashSet<AbilityId>,
    ) {
        if !visited.insert(id) {
            return;
        }
        if let Some(info) = self.abilities.get(&id) {
            for dep in &info.dependencies {
                self.collect_dependencies(*dep, result, visited);
                if !result.contains(dep) {
                    result.push(*dep);
                }
            }
        }
    }

    /// Get the ability set including an ability and all its dependencies.
    #[must_use]
    pub fn ability_with_dependencies(&self, id: AbilityId) -> AbilitySet {
        let mut abilities = vec![id];
        abilities.extend(self.transitive_dependencies(id));
        AbilitySet::from_abilities(abilities)
    }
}

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
    format!("{type_uuid}::{trait_name}::{method_name}").into()
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

    /// Create a registry pre-populated with the prelude traits.
    ///
    /// These are the operator traits (`Add`, `Sub`, `Mul`, `Div`, `Mod`,
    /// `Eq`, `Ord`) that operator overloading dispatches through. They're
    /// always in scope — `core_lib/traits.ab` mirrors these definitions for
    /// documentation and tooling. A module that declares its own trait with
    /// the same name shadows the prelude entry.
    #[must_use]
    pub fn with_prelude() -> Self {
        let mut registry = Self::new();
        let self_ty = || Type::Named(NamedType::simple("Self"));

        let binary = |registry: &mut Self, trait_name: &str, method: &str, ret: Type| {
            let id = registry.fresh_id();
            registry.register_trait(
                TraitDef::new(id, trait_name).with_method(TraitMethodDef::new(
                    method,
                    true,
                    vec![self_ty()],
                    ret,
                )),
            );
        };

        binary(&mut registry, "Add", "add", self_ty());
        binary(&mut registry, "Sub", "sub", self_ty());
        binary(&mut registry, "Mul", "mul", self_ty());
        binary(&mut registry, "Div", "div", self_ty());
        binary(&mut registry, "Mod", "rem", self_ty());
        binary(&mut registry, "Eq", "eq", Type::Bool);
        binary(&mut registry, "Ord", "cmp", Type::Number);

        registry
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

/// Represents a type in the Ambient language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    // ─────────────────────────────────────────────────────────────────────────
    // Primitive types
    // ─────────────────────────────────────────────────────────────────────────
    /// Unit type `()`, represents absence of a meaningful value.
    Unit,

    /// Boolean type `bool`.
    Bool,

    /// 64-bit floating point number (the only numeric type).
    Number,

    /// UTF-8 string type.
    String,

    /// Byte sequence type for binary data.
    Bytes,

    // ─────────────────────────────────────────────────────────────────────────
    // Composite types
    // ─────────────────────────────────────────────────────────────────────────
    /// Tuple type: fixed-size, heterogeneous collection.
    /// `(T1, T2, ..., Tn)`
    Tuple(Vec<Type>),

    /// Record type: named fields with types (structural typing).
    /// `{ field1: T1, field2: T2, ... }`
    Record(RecordType),

    /// Function type: parameters -> return type with abilities.
    /// `(P1, P2, ...) -> R with A1, A2, ...`
    Function(FunctionType),

    // ─────────────────────────────────────────────────────────────────────────
    // Polymorphism
    // ─────────────────────────────────────────────────────────────────────────
    /// A type variable used during inference.
    Var(TypeVar),

    /// A quantified (forall) type scheme.
    /// `forall a b. (a -> b) -> List<a> -> List<b>`
    Forall(ForallType),

    // ─────────────────────────────────────────────────────────────────────────
    // Named types
    // ─────────────────────────────────────────────────────────────────────────
    /// A named type constructor with optional type arguments.
    /// `List<T>`, `Option<T>`, `Map<K, V>`
    Named(NamedType),

    /// A nominal type distinguished by UUID, incompatible with structurally
    /// identical types.
    /// `unique(uuid) type UserId { value: string }`
    Nominal(NominalType),

    // ─────────────────────────────────────────────────────────────────────────
    // Ability types (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────
    /// A suspended ability value: `Ability<T, A!>`
    /// Represents an ability call that has been suspended and stored as a value.
    /// `T` is the result type when performed, `A!` is the ability required.
    AbilityValue(AbilityValueType),

    // ─────────────────────────────────────────────────────────────────────────
    // Handler types (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────
    /// A handler value type: `Handler<A>`
    /// Represents a first-class handler that can handle ability `A`.
    Handler(HandlerType),

    // ─────────────────────────────────────────────────────────────────────────
    // Special types
    // ─────────────────────────────────────────────────────────────────────────
    /// The never type `!`, for expressions that never return.
    Never,

    /// Error type used during type checking to allow recovery.
    Error,

    /// A type hole `_` for partial annotation.
    /// During inference, this is replaced with a fresh type variable.
    Hole,
}

/// A record type with named fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordType {
    /// Fields sorted by name for consistent comparison.
    pub fields: Vec<(Arc<str>, Type)>,
}

impl RecordType {
    /// Create a new record type with the given fields.
    /// Fields are sorted by name for consistent structural comparison.
    #[must_use]
    pub fn new(mut fields: Vec<(Arc<str>, Type)>) -> Self {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Self { fields }
    }

    /// Get the type of a field by name.
    #[must_use]
    pub fn get_field(&self, name: &str) -> Option<&Type> {
        self.fields
            .binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|idx| &self.fields[idx].1)
    }
}

/// A function type with parameters, return type, and ability requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    /// Parameter types.
    pub params: Vec<Type>,

    /// Return type.
    pub ret: Box<Type>,

    /// Abilities required by this function (Milestone 8).
    /// Empty means the function is pure.
    pub abilities: AbilitySet,
}

impl FunctionType {
    /// Create a new pure function type (no abilities).
    #[must_use]
    pub fn new(params: Vec<Type>, ret: Type) -> Self {
        Self {
            params,
            ret: Box::new(ret),
            abilities: AbilitySet::Empty,
        }
    }

    /// Create a new function type with abilities.
    #[must_use]
    pub fn with_abilities(params: Vec<Type>, ret: Type, abilities: AbilitySet) -> Self {
        Self {
            params,
            ret: Box::new(ret),
            abilities,
        }
    }

    /// Check if this function is pure (has no abilities).
    #[must_use]
    pub fn is_pure(&self) -> bool {
        self.abilities.is_pure()
    }
}

/// A suspended ability value type: `Ability<T, A!>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityValueType {
    /// The result type when the ability is performed.
    pub result: Box<Type>,

    /// The ability required to perform this suspended value.
    pub ability: AbilitySet,
}

impl AbilityValueType {
    /// Create a new ability value type.
    #[must_use]
    pub fn new(result: Type, ability: AbilitySet) -> Self {
        Self {
            result: Box::new(result),
            ability,
        }
    }
}

/// A handler value type: `Handler<A>`
///
/// Represents a first-class handler that can handle a specific ability.
/// Handler values can be passed around, stored, and composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerType {
    /// The ability that this handler handles.
    /// This is a single ability ID (not a set), as handlers handle one ability at a time.
    pub ability: AbilityId,
}

impl HandlerType {
    /// Create a new handler type.
    #[must_use]
    pub const fn new(ability: AbilityId) -> Self {
        Self { ability }
    }
}

/// A quantified type scheme (forall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForallType {
    /// Bound type variable IDs.
    pub vars: Vec<TypeVarId>,

    /// Bound ability variable IDs (Milestone 8).
    pub ability_vars: Vec<AbilityVarId>,

    /// The quantified type.
    pub body: Box<Type>,
}

impl ForallType {
    /// Create a new forall type.
    #[must_use]
    pub fn new(vars: Vec<TypeVarId>, body: Type) -> Self {
        Self {
            vars,
            ability_vars: Vec::new(),
            body: Box::new(body),
        }
    }

    /// Create a forall type with ability variables.
    #[must_use]
    pub fn with_abilities(
        vars: Vec<TypeVarId>,
        ability_vars: Vec<AbilityVarId>,
        body: Type,
    ) -> Self {
        Self {
            vars,
            ability_vars,
            body: Box::new(body),
        }
    }
}

/// A named type constructor (like `List<T>` or `Option<T>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    /// The type constructor name.
    pub name: Arc<str>,

    /// Type arguments (empty for non-generic types).
    pub args: Vec<Type>,
}

impl NamedType {
    /// Create a new named type.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }

    /// Create a non-generic named type.
    #[must_use]
    pub fn simple(name: impl Into<Arc<str>>) -> Self {
        Self::new(name, Vec::new())
    }
}

/// A nominal type distinguished by UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalType {
    /// Unique identifier making this type distinct from structurally
    /// identical types.
    pub uuid: Uuid,

    /// The underlying structural type.
    pub inner: Box<Type>,

    /// Optional human-readable name for error messages.
    pub name: Option<Arc<str>>,
}

impl NominalType {
    /// Create a new nominal type.
    #[must_use]
    pub fn new(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self {
            uuid,
            inner: Box::new(inner),
            name: name.map(Into::into),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type constructors (convenience methods)
// ─────────────────────────────────────────────────────────────────────────────

impl Type {
    /// Create a pure function type (no abilities).
    #[must_use]
    pub fn function(params: Vec<Type>, ret: Type) -> Self {
        Self::Function(FunctionType::new(params, ret))
    }

    /// Create a function type with abilities.
    #[must_use]
    pub fn function_with_abilities(params: Vec<Type>, ret: Type, abilities: AbilitySet) -> Self {
        Self::Function(FunctionType::with_abilities(params, ret, abilities))
    }

    /// Create an ability value type: `Ability<T, A!>`.
    #[must_use]
    pub fn ability_value(result: Type, ability: AbilitySet) -> Self {
        Self::AbilityValue(AbilityValueType::new(result, ability))
    }

    /// Create a handler type: `Handler<A>`.
    #[must_use]
    pub fn handler(ability: AbilityId) -> Self {
        Self::Handler(HandlerType::new(ability))
    }

    /// Create a tuple type.
    #[must_use]
    pub fn tuple(elements: Vec<Type>) -> Self {
        Self::Tuple(elements)
    }

    /// Create a record type.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Arc<str>>, Type)>) -> Self {
        let fields: Vec<_> = fields.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Self::Record(RecordType::new(fields))
    }

    /// Create a named type with arguments.
    #[must_use]
    pub fn named(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self::Named(NamedType::new(name, args))
    }

    /// Create a simple named type (no arguments).
    #[must_use]
    pub fn named_simple(name: impl Into<Arc<str>>) -> Self {
        Self::Named(NamedType::simple(name))
    }

    /// Create an `Option<T>` type.
    #[must_use]
    pub fn option(inner: Type) -> Self {
        Self::named("Option", vec![inner])
    }

    /// Create a `Result<T, E>` type.
    #[must_use]
    pub fn result(ok: Type, err: Type) -> Self {
        Self::named("Result", vec![ok, err])
    }

    /// Check if this type is `Option<T>` and return the inner type.
    #[must_use]
    pub fn as_option(&self) -> Option<&Type> {
        match self {
            Self::Named(n) if n.name.as_ref() == "Option" && n.args.len() == 1 => Some(&n.args[0]),
            _ => None,
        }
    }

    /// Check if this type is `Result<T, E>` and return the ok and error types.
    #[must_use]
    pub fn as_result(&self) -> Option<(&Type, &Type)> {
        match self {
            Self::Named(n) if n.name.as_ref() == "Result" && n.args.len() == 2 => {
                Some((&n.args[0], &n.args[1]))
            }
            _ => None,
        }
    }

    /// Check if this type is a `List<T>` and return the element type.
    #[must_use]
    pub fn as_list(&self) -> Option<&Type> {
        match self {
            Self::Named(n) if n.name.as_ref() == "List" && n.args.len() == 1 => Some(&n.args[0]),
            _ => None,
        }
    }

    /// Create an unbound type variable.
    #[must_use]
    pub fn var(id: TypeVarId) -> Self {
        Self::Var(TypeVar::Unbound(id))
    }

    /// Create a nominal type.
    #[must_use]
    pub fn nominal(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self::Nominal(NominalType::new(uuid, inner, name))
    }

    /// Create a forall (polymorphic) type.
    #[must_use]
    pub fn forall(vars: Vec<TypeVarId>, body: Type) -> Self {
        if vars.is_empty() {
            body
        } else {
            Self::Forall(ForallType::new(vars, body))
        }
    }

    /// Check if this type is a concrete (non-variable) type.
    #[must_use]
    pub fn is_concrete(&self) -> bool {
        match self {
            Self::Var(_) | Self::Hole => false,
            Self::Tuple(elems) => elems.iter().all(Type::is_concrete),
            Self::Record(rec) => rec.fields.iter().all(|(_, t)| t.is_concrete()),
            Self::Function(f) => {
                f.params.iter().all(Type::is_concrete)
                    && f.ret.is_concrete()
                    && f.abilities.ability_var().is_none()
            }
            Self::Named(n) => n.args.iter().all(Type::is_concrete),
            Self::Nominal(n) => n.inner.is_concrete(),
            Self::Forall(f) => f.body.is_concrete(),
            Self::AbilityValue(av) => av.result.is_concrete() && av.ability.ability_var().is_none(),
            // Handler types and all other types are concrete by default
            _ => true,
        }
    }

    /// Resolve type variable links to get the actual type.
    /// This follows chains of linked type variables.
    #[must_use]
    pub fn resolve(&self) -> Type {
        match self {
            Self::Var(TypeVar::Link(link)) => link.borrow().resolve(),
            other => other.clone(),
        }
    }

    /// Collect all free type variables in this type.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars);
        vars.sort_unstable();
        vars.dedup();
        vars
    }

    fn collect_free_vars(&self, vars: &mut Vec<TypeVarId>) {
        match self {
            Self::Var(TypeVar::Unbound(id)) => vars.push(*id),
            Self::Var(TypeVar::Link(link)) => link.borrow().collect_free_vars(vars),
            Self::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_vars(vars);
                }
            }
            Self::Record(rec) => {
                for (_, t) in &rec.fields {
                    t.collect_free_vars(vars);
                }
            }
            Self::Function(f) => {
                for p in &f.params {
                    p.collect_free_vars(vars);
                }
                f.ret.collect_free_vars(vars);
            }
            Self::Named(n) => {
                for arg in &n.args {
                    arg.collect_free_vars(vars);
                }
            }
            Self::Nominal(n) => n.inner.collect_free_vars(vars),
            Self::AbilityValue(av) => av.result.collect_free_vars(vars),
            Self::Forall(f) => {
                // Bound variables are not free
                let mut body_vars = Vec::new();
                f.body.collect_free_vars(&mut body_vars);
                for var in body_vars {
                    if !f.vars.contains(&var) {
                        vars.push(var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Collect all free ability variables in this type.
    #[must_use]
    pub fn free_ability_vars(&self) -> Vec<AbilityVarId> {
        let mut vars = Vec::new();
        self.collect_free_ability_vars(&mut vars);
        vars.sort_unstable();
        vars.dedup();
        vars
    }

    fn collect_free_ability_vars(&self, vars: &mut Vec<AbilityVarId>) {
        match self {
            Self::Function(f) => {
                for p in &f.params {
                    p.collect_free_ability_vars(vars);
                }
                f.ret.collect_free_ability_vars(vars);
                vars.extend(f.abilities.free_ability_vars());
            }
            Self::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_ability_vars(vars);
                }
            }
            Self::Record(rec) => {
                for (_, t) in &rec.fields {
                    t.collect_free_ability_vars(vars);
                }
            }
            Self::Named(n) => {
                for arg in &n.args {
                    arg.collect_free_ability_vars(vars);
                }
            }
            Self::Nominal(n) => n.inner.collect_free_ability_vars(vars),
            Self::AbilityValue(av) => {
                av.result.collect_free_ability_vars(vars);
                vars.extend(av.ability.free_ability_vars());
            }
            Self::Forall(f) => {
                let mut body_vars = Vec::new();
                f.body.collect_free_ability_vars(&mut body_vars);
                for var in body_vars {
                    if !f.ability_vars.contains(&var) {
                        vars.push(var);
                    }
                }
            }
            Self::Var(TypeVar::Link(link)) => link.borrow().collect_free_ability_vars(vars),
            _ => {}
        }
    }

    /// Substitute type variables with other types.
    #[must_use]
    pub fn substitute(&self, subst: &HashMap<TypeVarId, Type>) -> Type {
        self.substitute_all(subst, &HashMap::new())
    }

    /// Substitute both type variables and ability variables.
    #[must_use]
    pub fn substitute_all(
        &self,
        type_subst: &HashMap<TypeVarId, Type>,
        ability_subst: &HashMap<AbilityVarId, AbilitySet>,
    ) -> Type {
        match self {
            Self::Var(TypeVar::Unbound(id)) => {
                type_subst.get(id).cloned().unwrap_or_else(|| self.clone())
            }
            Self::Var(TypeVar::Link(link)) => {
                link.borrow().substitute_all(type_subst, ability_subst)
            }
            Self::Tuple(elems) => Self::Tuple(
                elems
                    .iter()
                    .map(|e| e.substitute_all(type_subst, ability_subst))
                    .collect(),
            ),
            Self::Record(rec) => Self::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.substitute_all(type_subst, ability_subst)))
                    .collect(),
            )),
            Self::Function(f) => {
                let new_abilities = substitute_ability_set(&f.abilities, ability_subst);
                Self::Function(FunctionType::with_abilities(
                    f.params
                        .iter()
                        .map(|p| p.substitute_all(type_subst, ability_subst))
                        .collect(),
                    f.ret.substitute_all(type_subst, ability_subst),
                    new_abilities,
                ))
            }
            Self::Named(n) => Self::Named(NamedType::new(
                n.name.clone(),
                n.args
                    .iter()
                    .map(|a| a.substitute_all(type_subst, ability_subst))
                    .collect(),
            )),
            Self::Nominal(n) => Self::Nominal(NominalType::new(
                n.uuid,
                n.inner.substitute_all(type_subst, ability_subst),
                n.name.clone(),
            )),
            Self::AbilityValue(av) => {
                let new_ability = substitute_ability_set(&av.ability, ability_subst);
                Self::AbilityValue(AbilityValueType::new(
                    av.result.substitute_all(type_subst, ability_subst),
                    new_ability,
                ))
            }
            Self::Forall(f) => {
                // Don't substitute bound variables
                let mut new_type_subst = type_subst.clone();
                for var in &f.vars {
                    new_type_subst.remove(var);
                }
                let mut new_ability_subst = ability_subst.clone();
                for var in &f.ability_vars {
                    new_ability_subst.remove(var);
                }
                Self::Forall(ForallType::with_abilities(
                    f.vars.clone(),
                    f.ability_vars.clone(),
                    f.body.substitute_all(&new_type_subst, &new_ability_subst),
                ))
            }
            _ => self.clone(),
        }
    }
}

/// Substitute ability variables in an ability set.
fn substitute_ability_set(
    ability_set: &AbilitySet,
    subst: &HashMap<AbilityVarId, AbilitySet>,
) -> AbilitySet {
    match ability_set {
        AbilitySet::Empty | AbilitySet::Concrete(_) | AbilitySet::Unresolved(_) => {
            ability_set.clone()
        }
        AbilitySet::Var(id) => subst
            .get(id)
            .cloned()
            .unwrap_or_else(|| ability_set.clone()),
        AbilitySet::Row { concrete, tail } => {
            if let Some(tail_set) = subst.get(tail) {
                // Merge concrete with the substituted tail
                AbilitySet::from_abilities(concrete.iter().copied()).union(tail_set)
            } else {
                ability_set.clone()
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Display implementations for pretty printing
// ─────────────────────────────────────────────────────────────────────────────

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(f, "()"),
            Self::Bool => write!(f, "bool"),
            Self::Number => write!(f, "number"),
            Self::String => write!(f, "string"),
            Self::Bytes => write!(f, "Bytes"),
            Self::Never => write!(f, "!"),
            Self::Error => write!(f, "<error>"),
            Self::Hole => write!(f, "_"),

            Self::Var(TypeVar::Unbound(id)) => write!(f, "'{id}"),
            Self::Var(TypeVar::Link(link)) => write!(f, "{}", link.borrow()),

            Self::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{elem}")?;
                }
                write!(f, ")")
            }

            Self::Record(rec) => {
                write!(f, "{{ ")?;
                for (i, (name, ty)) in rec.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                write!(f, " }}")
            }

            Self::Function(func) => {
                write!(f, "(")?;
                for (i, param) in func.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {}", func.ret)?;
                if !func.abilities.is_empty() {
                    write!(f, " with {}", func.abilities)?;
                }
                Ok(())
            }

            Self::Named(named) => {
                write!(f, "{}", named.name)?;
                if !named.args.is_empty() {
                    write!(f, "<")?;
                    for (i, arg) in named.args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{arg}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }

            Self::Nominal(nom) => {
                if let Some(name) = &nom.name {
                    write!(f, "{name}")
                } else {
                    write!(f, "unique({})", nom.uuid)
                }
            }

            Self::AbilityValue(av) => {
                write!(f, "Ability<{}, {}>", av.result, av.ability)
            }

            Self::Handler(handler) => {
                write!(f, "Handler<#{}>", handler.ability)
            }

            Self::Forall(forall) => {
                write!(f, "forall ")?;
                for (i, var) in forall.vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "'{var}")?;
                }
                for (i, var) in forall.ability_vars.iter().enumerate() {
                    if !forall.vars.is_empty() || i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "E{var}!")?;
                }
                write!(f, ". {}", forall.body)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct, recognizable AbilityId for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    #[test]
    fn test_primitive_types_display() {
        assert_eq!(Type::Unit.to_string(), "()");
        assert_eq!(Type::Bool.to_string(), "bool");
        assert_eq!(Type::Number.to_string(), "number");
        assert_eq!(Type::String.to_string(), "string");
        assert_eq!(Type::Never.to_string(), "!");
    }

    #[test]
    fn test_tuple_type_display() {
        let tuple = Type::tuple(vec![Type::Number, Type::String]);
        assert_eq!(tuple.to_string(), "(number, string)");
    }

    #[test]
    fn test_record_type_display() {
        let record = Type::record([("x", Type::Number), ("y", Type::Number)]);
        assert_eq!(record.to_string(), "{ x: number, y: number }");
    }

    #[test]
    fn test_function_type_display() {
        let func = Type::function(vec![Type::Number, Type::Number], Type::Number);
        assert_eq!(func.to_string(), "(number, number) -> number");
    }

    #[test]
    fn test_named_type_display() {
        let list = Type::named("List", vec![Type::Number]);
        assert_eq!(list.to_string(), "List<number>");

        let map = Type::named("Map", vec![Type::String, Type::Number]);
        assert_eq!(map.to_string(), "Map<string, number>");
    }

    #[test]
    fn test_type_var_display() {
        let var = Type::var(0);
        assert_eq!(var.to_string(), "'0");
    }

    #[test]
    fn test_forall_type_display() {
        let forall = Type::forall(vec![0, 1], Type::function(vec![Type::var(0)], Type::var(1)));
        assert_eq!(forall.to_string(), "forall '0 '1. ('0) -> '1");
    }

    #[test]
    fn test_type_var_generator() {
        let mut r#gen = TypeVarGen::new();
        let v1 = r#gen.fresh();
        let v2 = r#gen.fresh();
        let v3 = r#gen.fresh();

        assert_eq!(v1, Type::var(0));
        assert_eq!(v2, Type::var(1));
        assert_eq!(v3, Type::var(2));
    }

    #[test]
    fn test_record_field_access() {
        let record =
            if let Type::Record(rec) = Type::record([("x", Type::Number), ("y", Type::String)]) {
                rec
            } else {
                panic!("Expected record type");
            };

        assert_eq!(record.get_field("x"), Some(&Type::Number));
        assert_eq!(record.get_field("y"), Some(&Type::String));
        assert_eq!(record.get_field("z"), None);
    }

    #[test]
    fn test_free_vars() {
        let t = Type::function(vec![Type::var(0)], Type::var(1));
        let vars = t.free_vars();
        assert_eq!(vars, vec![0, 1]);
    }

    #[test]
    fn test_free_vars_in_forall() {
        // forall '0. ('0 -> '1) should have '1 free, '0 bound
        let t = Type::forall(vec![0], Type::function(vec![Type::var(0)], Type::var(1)));
        let vars = t.free_vars();
        assert_eq!(vars, vec![1]);
    }

    #[test]
    fn test_substitute() {
        let t = Type::function(vec![Type::var(0)], Type::var(1));
        let mut subst = HashMap::new();
        subst.insert(0, Type::Number);
        subst.insert(1, Type::String);

        let result = t.substitute(&subst);
        assert_eq!(result, Type::function(vec![Type::Number], Type::String));
    }

    #[test]
    fn test_is_concrete() {
        assert!(Type::Number.is_concrete());
        assert!(Type::function(vec![Type::Number], Type::String).is_concrete());
        assert!(!Type::var(0).is_concrete());
        assert!(!Type::function(vec![Type::var(0)], Type::Number).is_concrete());
    }

    #[test]
    fn test_nominal_type_inequality() {
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();

        let nominal1 = Type::nominal(uuid1, Type::String, Some("UserId"));
        let nominal2 = Type::nominal(uuid2, Type::String, Some("OrderId"));

        // Same structure, different UUIDs -> different types
        assert_ne!(nominal1, nominal2);
    }

    #[test]
    fn test_nominal_type_equality() {
        let uuid = Uuid::new_v4();

        let nominal1 = Type::nominal(uuid, Type::String, Some("UserId"));
        let nominal2 = Type::nominal(uuid, Type::String, Some("UserId"));

        // Same UUID -> same type
        assert_eq!(nominal1, nominal2);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Ability type tests (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_ability_set_empty() {
        let empty = AbilitySet::empty();
        assert!(empty.is_empty());
        assert!(empty.is_pure());
        assert!(!empty.contains(aid(1)));
        assert_eq!(empty.to_string(), "{}");
    }

    #[test]
    fn test_ability_set_single() {
        let single = AbilitySet::single(aid(1));
        assert!(!single.is_empty());
        assert!(!single.is_pure());
        assert!(single.contains(aid(1)));
        assert!(!single.contains(aid(2)));
        assert_eq!(single.to_string(), format!("{{#{}}}", aid(1)));
    }

    #[test]
    fn test_ability_set_from_abilities() {
        let abilities = AbilitySet::from_abilities([aid(3), aid(1), aid(2), aid(1)]); // duplicates should be removed
        assert!(abilities.contains(aid(1)));
        assert!(abilities.contains(aid(2)));
        assert!(abilities.contains(aid(3)));
        assert!(!abilities.contains(aid(4)));
        // Should be sorted
        assert_eq!(abilities.concrete_abilities(), &[aid(1), aid(2), aid(3)]);
    }

    #[test]
    fn test_ability_set_var() {
        let var = AbilitySet::var(42);
        assert!(!var.is_empty());
        assert!(!var.is_pure());
        assert_eq!(var.ability_var(), Some(42));
        assert_eq!(var.to_string(), "E42!");
    }

    #[test]
    fn test_ability_set_row() {
        let row = AbilitySet::row([aid(1), aid(2)], 99);
        assert!(!row.is_empty());
        assert!(row.contains(aid(1)));
        assert!(row.contains(aid(2)));
        assert_eq!(row.ability_var(), Some(99));
        assert_eq!(
            row.to_string(),
            format!("{{#{}, #{}, E99!}}", aid(1), aid(2))
        );
    }

    #[test]
    fn test_ability_set_union() {
        let a = AbilitySet::from_abilities([aid(1), aid(2)]);
        let b = AbilitySet::from_abilities([aid(2), aid(3)]);
        let union = a.union(&b);

        if let AbilitySet::Concrete(abilities) = union {
            assert_eq!(abilities, vec![aid(1), aid(2), aid(3)]);
        } else {
            panic!("Expected concrete ability set");
        }
    }

    #[test]
    fn test_ability_set_union_with_var() {
        let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);
        let var = AbilitySet::var(0);
        let union = concrete.union(&var);

        if let AbilitySet::Row { concrete, tail } = union {
            assert_eq!(concrete, vec![aid(1), aid(2)]);
            assert_eq!(tail, 0);
        } else {
            panic!("Expected row ability set");
        }
    }

    #[test]
    fn test_ability_set_free_vars() {
        let empty = AbilitySet::empty();
        assert!(empty.free_ability_vars().is_empty());

        let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);
        assert!(concrete.free_ability_vars().is_empty());

        let var = AbilitySet::var(5);
        assert_eq!(var.free_ability_vars(), vec![5]);

        let row = AbilitySet::row([aid(1), aid(2)], 10);
        assert_eq!(row.free_ability_vars(), vec![10]);
    }

    #[test]
    fn test_ability_value_type() {
        let av = Type::ability_value(Type::String, AbilitySet::single(aid(1)));
        assert_eq!(av.to_string(), format!("Ability<string, {{#{}}}>", aid(1)));

        if let Type::AbilityValue(avt) = av {
            assert_eq!(*avt.result, Type::String);
            assert!(avt.ability.contains(aid(1)));
        } else {
            panic!("Expected AbilityValue type");
        }
    }

    #[test]
    fn test_function_with_abilities() {
        let func = Type::function_with_abilities(
            vec![Type::String],
            Type::Unit,
            AbilitySet::from_abilities([aid(1), aid(2)]),
        );

        assert_eq!(
            func.to_string(),
            format!("(string) -> () with {{#{}, #{}}}", aid(1), aid(2))
        );

        if let Type::Function(ft) = func {
            assert!(!ft.is_pure());
            assert!(ft.abilities.contains(aid(1)));
            assert!(ft.abilities.contains(aid(2)));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_pure_function() {
        let func = Type::function(vec![Type::Number], Type::Number);

        if let Type::Function(ft) = func {
            assert!(ft.is_pure());
            assert!(ft.abilities.is_empty());
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_ability_var_generator() {
        let mut r#gen = TypeVarGen::new();
        let v1 = r#gen.fresh_ability_var();
        let v2 = r#gen.fresh_ability_var();

        assert_eq!(v1, AbilitySet::Var(0));
        assert_eq!(v2, AbilitySet::Var(1));
    }

    #[test]
    fn test_forall_with_ability_vars() {
        let forall = Type::Forall(ForallType::with_abilities(
            vec![0],
            vec![1],
            Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1)),
        ));

        assert_eq!(forall.to_string(), "forall '0 E1!. ('0) -> '0 with E1!");
    }

    #[test]
    fn test_ability_value_is_not_concrete() {
        let av = Type::ability_value(Type::String, AbilitySet::var(0));
        assert!(!av.is_concrete());

        let av_concrete = Type::ability_value(Type::String, AbilitySet::single(aid(1)));
        assert!(av_concrete.is_concrete());
    }

    #[test]
    fn test_function_with_ability_var_is_not_concrete() {
        let func =
            Type::function_with_abilities(vec![Type::Number], Type::Number, AbilitySet::var(0));
        assert!(!func.is_concrete());
    }

    #[test]
    fn test_free_ability_vars_in_function() {
        let func =
            Type::function_with_abilities(vec![Type::Number], Type::Number, AbilitySet::var(42));
        assert_eq!(func.free_ability_vars(), vec![42]);
    }

    #[test]
    fn test_free_ability_vars_in_ability_value() {
        let av = Type::ability_value(Type::String, AbilitySet::var(10));
        assert_eq!(av.free_ability_vars(), vec![10]);
    }

    #[test]
    fn test_substitute_ability_vars() {
        let func =
            Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1));

        let type_subst: HashMap<TypeVarId, Type> = [(0, Type::Number)].into_iter().collect();
        let ability_subst: HashMap<AbilityVarId, AbilitySet> =
            [(1, AbilitySet::single(aid(99)))].into_iter().collect();

        let result = func.substitute_all(&type_subst, &ability_subst);

        if let Type::Function(ft) = result {
            assert_eq!(ft.params, vec![Type::Number]);
            assert_eq!(*ft.ret, Type::Number);
            assert_eq!(ft.abilities, AbilitySet::single(aid(99)));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_type_hole_display() {
        assert_eq!(Type::Hole.to_string(), "_");
    }

    #[test]
    fn test_type_hole_is_not_concrete() {
        assert!(!Type::Hole.is_concrete());
        // Hole in nested type
        assert!(!Type::function(vec![Type::Hole], Type::Number).is_concrete());
        assert!(!Type::Tuple(vec![Type::Number, Type::Hole]).is_concrete());
    }

    #[test]
    fn test_ability_registry_basic() {
        let mut registry = AbilityRegistry::new();

        let info = AbilityInfo::new("Console").with_method("print", vec![Type::String], Type::Unit);

        registry.register(aid(1), info);

        assert!(registry.get(aid(1)).is_some());
        assert_eq!(registry.lookup("Console"), Some(aid(1)));
        assert_eq!(registry.lookup("Unknown"), None);
    }

    #[test]
    fn test_ability_registry_dependencies() {
        let mut registry = AbilityRegistry::new();

        // IO is a base ability
        registry.register(aid(1), AbilityInfo::new("IO"));

        // FileSystem depends on IO
        registry.register(
            aid(2),
            AbilityInfo::new("FileSystem").with_dependency(aid(1)),
        );

        // Database depends on IO
        registry.register(aid(3), AbilityInfo::new("Database").with_dependency(aid(1)));

        // App depends on FileSystem and Database
        registry.register(
            aid(4),
            AbilityInfo::new("App")
                .with_dependency(aid(2))
                .with_dependency(aid(3)),
        );

        // Check transitive dependencies
        assert!(registry.transitive_dependencies(aid(1)).is_empty());
        assert_eq!(registry.transitive_dependencies(aid(2)), vec![aid(1)]);
        assert_eq!(registry.transitive_dependencies(aid(3)), vec![aid(1)]);

        // App should transitively depend on IO via both FileSystem and Database
        let app_deps = registry.transitive_dependencies(aid(4));
        assert!(app_deps.contains(&aid(1))); // IO
        assert!(app_deps.contains(&aid(2))); // FileSystem
        assert!(app_deps.contains(&aid(3))); // Database
    }

    #[test]
    fn test_ability_with_dependencies() {
        let mut registry = AbilityRegistry::new();

        registry.register(aid(1), AbilityInfo::new("IO"));
        registry.register(
            aid(2),
            AbilityInfo::new("FileSystem").with_dependency(aid(1)),
        );

        let set = registry.ability_with_dependencies(aid(2));

        // Should include both FileSystem (2) and IO (1)
        if let AbilitySet::Concrete(abilities) = set {
            assert!(abilities.contains(&aid(1)));
            assert!(abilities.contains(&aid(2)));
        } else {
            panic!("Expected concrete ability set");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Option and Result type tests (Milestone 15)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_option_type() {
        let opt_num = Type::option(Type::Number);
        assert_eq!(opt_num.to_string(), "Option<number>");

        // Check as_option works
        assert_eq!(opt_num.as_option(), Some(&Type::Number));

        // Non-option types return None
        assert_eq!(Type::Number.as_option(), None);
        assert_eq!(Type::named("List", vec![Type::Number]).as_option(), None);
    }

    #[test]
    fn test_result_type() {
        let res = Type::result(Type::String, Type::Number);
        assert_eq!(res.to_string(), "Result<string, number>");

        // Check as_result works
        assert_eq!(res.as_result(), Some((&Type::String, &Type::Number)));

        // Non-result types return None
        assert_eq!(Type::Number.as_result(), None);
        assert_eq!(Type::option(Type::Number).as_result(), None);
    }

    #[test]
    fn test_as_list() {
        let list = Type::named("List", vec![Type::Number]);
        assert_eq!(list.as_list(), Some(&Type::Number));

        // Non-list types return None
        assert_eq!(Type::Number.as_list(), None);
        assert_eq!(Type::option(Type::Number).as_list(), None);
    }

    #[test]
    fn test_nested_option_result() {
        // Option<Result<number, string>>
        let nested = Type::option(Type::result(Type::Number, Type::String));
        assert_eq!(nested.to_string(), "Option<Result<number, string>>");

        // Check we can extract inner types
        if let Some(inner) = nested.as_option() {
            if let Some((ok, err)) = inner.as_result() {
                assert_eq!(ok, &Type::Number);
                assert_eq!(err, &Type::String);
            } else {
                panic!("Expected Result inside Option");
            }
        } else {
            panic!("Expected Option type");
        }
    }
}
