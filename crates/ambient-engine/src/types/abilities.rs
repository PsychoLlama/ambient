//! Ability sets ([`AbilitySet`]) and the ability registry.

use std::collections::HashMap;
use std::sync::Arc;

use super::{AbilityId, AbilityVarId, Type};

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
    /// IDs yet (e.g. `(T) -> U with Stdio`). Produced by lowering, which
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

    /// Display-only: collapse an unconstrained effect row to its known part.
    ///
    /// A generalized-but-uninstantiated effect row — a bare [`Self::Var`], or a
    /// [`Self::Row`] whose tail is still an unconstrained variable — carries no
    /// information a reader can use: it means "no *known* effects", not "some
    /// unknown effect". Hover and completions render it as its concrete part
    /// only (so `with E23!` disappears), never as `E{id}!`. This is a view
    /// transform for the analysis-layer formatters; unification, hashing, and
    /// diagnostics keep the raw set, where a row variable is meaningful.
    #[must_use]
    pub fn closed(&self) -> Self {
        match self {
            Self::Var(_) => Self::Empty,
            Self::Row { concrete, .. } => Self::from_abilities(concrete.iter().copied()),
            Self::Empty | Self::Concrete(_) | Self::Unresolved(_) => self.clone(),
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
