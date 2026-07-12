//! Type environment for tracking bindings and their types.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::fqn::{Fqn, NameKey};
use crate::types::{AbilityVarId, Type, TypeVarId};

/// A type environment mapping bindings to their types.
///
/// Uses a persistent structure for efficient scoping.
#[derive(Debug, Clone, Default)]
pub struct TypeEnv {
    /// Mapping from binding IDs to type schemes.
    bindings: HashMap<BindingId, Scheme>,

    /// Mapping from lookup keys to binding IDs. Locals and same-module
    /// items key by [`NameKey::Bare`] (their bare source name);
    /// cross-module imports key by [`NameKey::Item`] (their [`Fqn`]).
    names: HashMap<NameKey, BindingId>,
}

/// What a registered type name denotes — the value of the checker's
/// alias table (`Infer::type_aliases`).
///
/// Names enter the table through exactly one rule, [`AliasTarget::of_struct`]
/// (plus `type` aliases, always [`Whole`](Self::Whole)), shared by every
/// registration channel: local declarations, foreign package types, and the
/// prelude seeding used by ability resolution. There is deliberately no
/// name-keyed builtin fallback behind this table — a type name resolves iff
/// something in scope put it here.
#[derive(Debug, Clone)]
pub enum AliasTarget {
    /// Expand the name to this type: a `type` alias's target, a struct's
    /// body (`Type::Nominal` when `unique`), a primitive's nominal. Written
    /// type arguments never substitute into it, so only a bare (nullary)
    /// reference expands.
    Whole(Type),
    /// An opaque generic head — `extern unique(u) struct Foo<T, …>;`. The
    /// declaration has no body, so its parameters are phantom and there is
    /// nothing to expand: the applied form is `Named(name, args, uuid)`,
    /// identity plus arguments (the same shape an enum reference takes).
    /// `List`/`Map`/`Set` resolve through this.
    OpaqueGeneric {
        /// The declaration's `unique(…)` nominal identity.
        uuid: uuid::Uuid,
        /// Declared type-parameter count (`Map<K, V>` → 2). Enforced by
        /// unification (arity is part of a `Named`'s shape), not at the
        /// annotation site.
        arity: usize,
    },
    /// A declared struct with a *record body* and one or more named type
    /// parameters (`struct Box<A, B> { a: A, b: B }`). Unlike an opaque
    /// generic head, its parameters are real: they appear in the body as bare
    /// `Named(param)` placeholders. Applying arguments substitutes
    /// params→args into the body, yielding the concrete `Type::Nominal`
    /// record — the same shape a non-generic struct's `Whole` body already is
    /// (so projection, unification, and inherent-impl keying reuse the nominal
    /// machinery). A bare (nullary) reference expands to `body` with its
    /// params free. Non-generic structs still register as [`Whole`](Self::Whole).
    GenericStruct {
        /// Declared type parameters, in order, so applied args zip
        /// positionally.
        type_params: Vec<Arc<str>>,
        /// The struct body: `Type::Nominal(uuid, Record{…})` for a `unique`
        /// struct, a bare `Type::Record` for a structural one, with fields
        /// written in terms of `type_params`.
        body: Type,
    },
}

impl AliasTarget {
    /// The alias-table target for a struct declaration — the single rule
    /// every registration channel shares. An `extern` unit struct *with*
    /// type parameters and a `unique(…)` identity is an opaque generic
    /// head; every other struct registers its body type.
    #[must_use]
    pub fn of_struct(def: &crate::ast::StructDef) -> Self {
        match def.unique_id {
            Some(uuid) if !def.type_params.is_empty() && def.is_extern && def.is_unit() => {
                Self::OpaqueGeneric {
                    uuid,
                    arity: def.type_params.len(),
                }
            }
            // A fielded generic struct: its parameters are real body
            // placeholders, applied by substitution in the checker.
            _ if !def.type_params.is_empty() => Self::GenericStruct {
                type_params: def
                    .type_params
                    .iter()
                    .map(|tp| Arc::clone(&tp.name))
                    .collect(),
                body: def.ty.clone(),
            },
            _ => Self::Whole(def.ty.clone()),
        }
    }

    /// The type this name expands to when written bare, if it is an
    /// expansion-style entry.
    #[must_use]
    pub fn whole(&self) -> Option<&Type> {
        match self {
            Self::Whole(ty) | Self::GenericStruct { body: ty, .. } => Some(ty),
            Self::OpaqueGeneric { .. } => None,
        }
    }

    /// The uuid this name's inherent/trait impls key on, if it denotes a
    /// nominal type.
    #[must_use]
    pub fn impl_uuid(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Whole(Type::Nominal(n))
            | Self::GenericStruct {
                body: Type::Nominal(n),
                ..
            } => Some(n.uuid),
            Self::OpaqueGeneric { uuid, .. } => Some(*uuid),
            Self::Whole(_) | Self::GenericStruct { .. } => None,
        }
    }
}

/// A type scheme (potentially polymorphic type).
///
/// `forall a b E!. T` where `a` and `b` are quantified type variables
/// and `E!` is a quantified ability variable.
#[derive(Debug, Clone)]
pub struct Scheme {
    /// Quantified type variables.
    pub vars: Vec<TypeVarId>,
    /// Quantified ability variables (Milestone 8).
    pub ability_vars: Vec<AbilityVarId>,
    /// Trait bounds on the quantified variables, in declaration order
    /// (`fn f<T: Eq + Ord, U: Eq>` → `[(T, Eq), (T, Ord), (U, Eq)]`).
    /// This order *is* the dictionary-parameter order: the compiled
    /// function takes one hidden trailing dictionary per entry, and call
    /// sites supply them in the same order.
    pub bounds: Vec<(TypeVarId, crate::types::TraitBound)>,
    /// The type body.
    pub ty: Type,
}

impl Scheme {
    /// Create a monomorphic scheme (no quantified variables).
    #[must_use]
    pub fn mono(ty: Type) -> Self {
        Self {
            vars: Vec::new(),
            ability_vars: Vec::new(),
            bounds: Vec::new(),
            ty,
        }
    }

    /// Create a polymorphic scheme with type variables only.
    #[must_use]
    pub fn poly(vars: Vec<TypeVarId>, ty: Type) -> Self {
        Self {
            vars,
            ability_vars: Vec::new(),
            bounds: Vec::new(),
            ty,
        }
    }

    /// Create a polymorphic scheme with both type and ability variables.
    #[must_use]
    pub fn poly_with_abilities(
        vars: Vec<TypeVarId>,
        ability_vars: Vec<AbilityVarId>,
        ty: Type,
    ) -> Self {
        Self {
            vars,
            ability_vars,
            bounds: Vec::new(),
            ty,
        }
    }

    /// Attach trait bounds (dictionary-parameter order) to this scheme.
    #[must_use]
    pub fn with_bounds(mut self, bounds: Vec<(TypeVarId, crate::types::TraitBound)>) -> Self {
        self.bounds = bounds;
        self
    }
}

impl TypeEnv {
    /// Create an empty type environment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding under its bare name (a local, or a same-module
    /// item).
    pub fn insert(&mut self, id: BindingId, name: Arc<str>, scheme: Scheme) {
        self.insert_key(id, NameKey::Bare(name), scheme);
    }

    /// Insert a binding under a cross-module item's [`Fqn`] identity.
    pub fn insert_item(&mut self, id: BindingId, fqn: Fqn, scheme: Scheme) {
        self.insert_key(id, NameKey::Item(fqn), scheme);
    }

    /// Insert a binding under an arbitrary [`NameKey`].
    pub fn insert_key(&mut self, id: BindingId, key: NameKey, scheme: Scheme) {
        // Re-binding a *name* is legal shadowing, but a binding id may
        // never be shared by two names: both would alias one `bindings`
        // slot and the last write would silently replace the other
        // name's scheme (the id channels in `check` allocate from
        // disjoint ranges precisely for this).
        debug_assert!(
            !self.bindings.contains_key(&id) || self.names.get(&key) == Some(&id),
            "binding id {id} is already taken by another name (inserting `{key:?}`)"
        );
        self.bindings.insert(id, scheme);
        self.names.insert(key, id);
    }

    /// Insert a binding with a monomorphic type under its bare name.
    pub fn insert_mono(&mut self, id: BindingId, name: Arc<str>, ty: Type) {
        self.insert(id, name, Scheme::mono(ty));
    }

    /// Look up a binding by ID.
    #[must_use]
    pub fn get(&self, id: BindingId) -> Option<&Scheme> {
        self.bindings.get(&id)
    }

    /// Look up a binding by its bare name (a local or same-module item).
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&Scheme> {
        self.get_key(&NameKey::Bare(Arc::from(name)))
    }

    /// Look up a binding by its [`NameKey`] — the single lookup convention
    /// a reference's [`crate::ast::QualifiedName::resolution_key`] targets.
    #[must_use]
    pub fn get_key(&self, key: &NameKey) -> Option<&Scheme> {
        self.names.get(key).and_then(|id| self.bindings.get(id))
    }

    /// Extend the environment with a new scope (for let bindings).
    #[must_use]
    pub fn extend(&self) -> Self {
        self.clone()
    }

    /// Collect all free type variables in the environment.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        for scheme in self.bindings.values() {
            let scheme_vars = scheme.ty.free_vars();
            for var in scheme_vars {
                if !scheme.vars.contains(&var) && !vars.contains(&var) {
                    vars.push(var);
                }
            }
        }
        vars
    }

    /// Collect all free ability variables in the environment.
    #[must_use]
    pub fn free_ability_vars(&self) -> Vec<AbilityVarId> {
        let mut vars = Vec::new();
        for scheme in self.bindings.values() {
            let scheme_vars = scheme.ty.free_ability_vars();
            for var in scheme_vars {
                if !scheme.ability_vars.contains(&var) && !vars.contains(&var) {
                    vars.push(var);
                }
            }
        }
        vars
    }

    /// Iterate over all bindings in the environment.
    pub fn iter(&self) -> impl Iterator<Item = (BindingId, &Scheme)> + '_ {
        self.bindings.iter().map(|(&id, scheme)| (id, scheme))
    }

    /// Iterate over all name keys in the environment.
    #[must_use]
    pub fn names(&self) -> &HashMap<NameKey, BindingId> {
        &self.names
    }
}
