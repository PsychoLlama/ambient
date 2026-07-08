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
            _ => Self::Whole(def.ty.clone()),
        }
    }

    /// The type this name expands to when written bare, if it is an
    /// expansion-style entry.
    #[must_use]
    pub fn whole(&self) -> Option<&Type> {
        match self {
            Self::Whole(ty) => Some(ty),
            Self::OpaqueGeneric { .. } => None,
        }
    }

    /// The uuid this name's inherent/trait impls key on, if it denotes a
    /// nominal type.
    #[must_use]
    pub fn impl_uuid(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Whole(Type::Nominal(n)) => Some(n.uuid),
            Self::OpaqueGeneric { uuid, .. } => Some(*uuid),
            Self::Whole(_) => None,
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
            ty,
        }
    }

    /// Create a polymorphic scheme with type variables only.
    #[must_use]
    pub fn poly(vars: Vec<TypeVarId>, ty: Type) -> Self {
        Self {
            vars,
            ability_vars: Vec::new(),
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
            ty,
        }
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
