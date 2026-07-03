//! Type environment for tracking bindings and their types.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::types::{AbilityVarId, Type, TypeVarId};

/// A type environment mapping bindings to their types.
///
/// Uses a persistent structure for efficient scoping.
#[derive(Debug, Clone, Default)]
pub struct TypeEnv {
    /// Mapping from binding IDs to type schemes.
    bindings: HashMap<BindingId, Scheme>,

    /// Mapping from names to binding IDs (for named lookups).
    names: HashMap<Arc<str>, BindingId>,
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

    /// Insert a binding with its type scheme.
    pub fn insert(&mut self, id: BindingId, name: Arc<str>, scheme: Scheme) {
        self.bindings.insert(id, scheme);
        self.names.insert(name, id);
    }

    /// Insert a binding with a monomorphic type.
    pub fn insert_mono(&mut self, id: BindingId, name: Arc<str>, ty: Type) {
        self.insert(id, name, Scheme::mono(ty));
    }

    /// Look up a binding by ID.
    #[must_use]
    pub fn get(&self, id: BindingId) -> Option<&Scheme> {
        self.bindings.get(&id)
    }

    /// Look up a binding by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&Scheme> {
        self.names.get(name).and_then(|id| self.bindings.get(id))
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

    /// Iterate over all names in the environment.
    #[must_use]
    pub fn names(&self) -> &HashMap<Arc<str>, BindingId> {
        &self.names
    }
}
