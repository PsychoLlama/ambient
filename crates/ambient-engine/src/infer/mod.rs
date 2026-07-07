//! Type inference for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with:
//! - Algorithm W for principal type inference
//! - Unification with occurs check
//! - Let-polymorphism (generalization at let bindings)
//! - Type environment with lexical scoping
//!
//! # Algorithm W Overview
//!
//! The inference algorithm works in two phases:
//!
//! 1. **Constraint Generation**: Traverse the AST, assigning fresh type variables
//!    to expressions and collecting equality constraints between types.
//!
//! 2. **Unification**: Solve constraints by finding a most general unifier (MGU).
//!    Uses substitution to replace type variables with concrete types.
//!
//! ## Key Operations
//!
//! - **`infer_expr`**: Infers the type of an expression, returning constraints
//! - **`unify`**: Unifies two types, updating the substitution map
//! - **`generalize`**: Converts a type to a polymorphic scheme (∀-quantified)
//! - **`instantiate`**: Creates fresh type variables for a polymorphic scheme
//!
//! ## Example: Let Polymorphism
//!
//! ```text
//! let id = |x| x;    // id : ∀a. a -> a
//! let a = id(42);    // instantiate: number -> number
//! let b = id(true);  // instantiate: bool -> bool
//! ```
//!
//! The identity function `id` is generalized at its binding site, allowing
//! it to be used at different types.
//!
//! # Ability Tracking
//!
//! The type system tracks algebraic effects through ability sets on function
//! types. A function `fn(): number / Stdio` can perform Stdio operations.
//! The inference engine propagates these ability requirements and checks that
//! all abilities are handled.
//!
//! # Module Organization
//!
//! - [`error`] - Type error types and display implementations
//! - [`env`] - Type environment (`TypeEnv`) and type schemes (`Scheme`)
//! - [`check`] - Module-level type checking (`check_module`)
//! - [`unify`] - Type and ability unification
//! - [`expr`] - Expression type inference
//! - [`pattern`] - Pattern matching inference
//! - [`intrinsics`] - Intrinsic function type inference
//! - [`abilities`] - Ability lookup and async type inference
//! - [`Infer`] - The main type inference engine

mod abilities;
mod check;
mod effects;
pub mod enums;
mod env;
mod error;
mod expr;
pub mod inherent;
mod intrinsics;
mod pattern;
mod unify;

pub use check::{
    CheckResult, check_module, check_module_with_registry, check_module_with_registry_and_resolver,
    check_module_with_resolver, resolve_ability_declarations, resolve_registry_abilities,
};
pub use env::{Scheme, TypeEnv};
pub use error::{BoxedTypeError, BoxedTypeErrorExt, InferResult, TypeError, TypeErrorKind};

use error::type_error;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ability_resolver::AbilityResolver;
use crate::fqn::NameKey;
use crate::types::{
    AbilityId, AbilityRegistry, AbilitySet, AbilityValueType, AbilityVarId, ForallType, NamedType,
    RecordType, TraitRegistry, Type, TypeVarGen, TypeVarId,
};

// ─────────────────────────────────────────────────────────────────────────────
// Type Inference
// ─────────────────────────────────────────────────────────────────────────────

/// Type inference context.
pub struct Infer {
    /// Type variable generator.
    r#gen: TypeVarGen,
    /// Substitution mapping type variables to their bindings.
    pub(crate) subst: HashMap<TypeVarId, Type>,
    /// Substitution mapping ability variables to their bindings (Milestone 8).
    pub(crate) ability_subst: HashMap<AbilityVarId, AbilitySet>,
    /// Current ability requirements being accumulated (Milestone 8).
    /// This tracks abilities used in the current function being inferred.
    pub(crate) current_abilities: AbilitySet,
    /// Optional ability registry for dependency tracking.
    pub(crate) ability_registry: Option<AbilityRegistry>,
    /// Ability resolver for looking up ability and method information.
    pub(crate) ability_resolver: AbilityResolver,
    /// Type alias registry for looking up types by name.
    /// Maps type alias names to their resolved types (including Nominal types).
    pub(crate) type_aliases: HashMap<NameKey, Type>,
    /// Trait registry for trait and impl lookup.
    pub(crate) trait_registry: TraitRegistry,
    /// Inherent (trait-less) impl methods, keyed by target type identity.
    pub(crate) inherent_registry: inherent::InherentRegistry,

    /// Enums visible to the module being checked (prelude + locals).
    pub(crate) enum_registry: enums::EnumRegistry,
    /// Errors recorded outside the normal `InferResult` flow (e.g. unknown
    /// ability names found while resolving annotations). Drained by the
    /// module-level check functions.
    pub(crate) pending_errors: Vec<error::BoxedTypeError>,
    /// Enclosing handler-arm contexts for typing `resume` (innermost last).
    pub(crate) resume_contexts: Vec<ResumeContext>,
    /// Handle expressions whose body effects were still polymorphic when
    /// the handle was checked. Resolved (handled abilities subtracted) by
    /// [`Infer::resolve_pending_discharges`] once every body in the module
    /// has been checked and ability variables are bound.
    pub(crate) pending_discharges: Vec<PendingDischarge>,
    /// Sandbox expressions whose body effects were still polymorphic when
    /// the sandbox was checked. Enforced by
    /// [`Infer::resolve_pending_sandbox_checks`] after discharges resolve.
    pub(crate) pending_sandbox_checks: Vec<PendingSandboxCheck>,
    /// The workspace package name user items are scoped under
    /// (`workspace::<name>`). Seeded from the registry per check so
    /// [`ModuleId`](crate::fqn::ModuleId)s the checker mints match the
    /// registry's. Empty when checking without a registry.
    pub(crate) workspace_name: Arc<str>,

    /// Type-parameter names rigid in the body currently being checked
    /// (`T` in `fn f<T>(...)`, plus an impl block's own params). While a
    /// name is in this set, [`resolve_holes`](Self::resolve_holes) rewrites a
    /// bare `Type::Named` of that name to [`Type::Param`] instead of leaving
    /// it as an unresolved nominal reference — the sole point rigid params
    /// are minted. Set by [`with_rigid_params`](Self::with_rigid_params)
    /// around body checking and empty everywhere else (signature-scheme
    /// paths substitute to `Var` instead), so it never affects hashing.
    pub(crate) rigid_params: HashSet<Arc<str>>,
}

/// A deferred "the sandbox body may only use these abilities" check.
#[derive(Debug)]
pub(crate) struct PendingSandboxCheck {
    /// The body's effect set, as of the sandbox site.
    pub body: AbilitySet,
    /// Resolved identities of the allowed abilities.
    pub allowed: Vec<AbilityId>,
    /// Allowed ability names as written, for the error message.
    pub allowed_names: Vec<Arc<str>>,
    /// The sandbox expression's span.
    pub span: (u32, u32),
}

/// What `resume` means inside the handler arm currently being checked.
#[derive(Debug, Clone)]
pub(crate) struct ResumeContext {
    /// Type of the value `resume` feeds to the continuation — the ability
    /// method's return type. `None` when unconstrainable: a method
    /// returning `!` (Exception's `throw`) can be raised by the host at
    /// any perform site, so the continuation's expected value is
    /// statically unknowable (resuming substitutes a value for the
    /// *failing call*, not for `throw` itself).
    pub value_ty: Option<Type>,
    /// Type of the `resume(...)` expression itself: the handle
    /// expression's result. `None` inside handler literals, where the
    /// eventual handle site is unknown.
    pub result_ty: Option<Type>,
}

/// A deferred "subtract handled abilities from this body's effects".
///
/// When a handle expression's body effects still contain unbound ability
/// variables (calls to functions whose effects bind later in the check
/// pass), the subtraction can't happen at the handle site. The handle
/// instead contributes a fresh `remainder` variable to its enclosing
/// context and records this, to be resolved once all bodies are checked.
#[derive(Debug)]
pub(crate) struct PendingDischarge {
    /// The handled body's effect set, as of the handle site.
    pub body: AbilitySet,
    /// Abilities the handle's arms and handler values cover.
    pub handled: Vec<AbilityId>,
    /// The variable standing in for "body effects minus handled".
    pub remainder: AbilityVarId,
}

impl Default for Infer {
    fn default() -> Self {
        Self::new()
    }
}

impl Infer {
    fn with_parts(registry: Option<AbilityRegistry>, resolver: AbilityResolver) -> Self {
        Self {
            r#gen: TypeVarGen::new(),
            subst: HashMap::new(),
            ability_subst: HashMap::new(),
            current_abilities: AbilitySet::Empty,
            ability_registry: registry,
            ability_resolver: resolver,
            // Option/Result, the primitive types, and the operator traits
            // are no longer seeded here: they enter every module through the
            // `core::prelude` injection (`register_imported_enums` /
            // `register_imported_traits` / the primitive imports), exactly
            // like any other import. A registry-less check (no prelude)
            // therefore starts without them.
            type_aliases: HashMap::new(),
            trait_registry: TraitRegistry::default(),
            inherent_registry: inherent::InherentRegistry::default(),
            enum_registry: enums::EnumRegistry::default(),
            pending_errors: Vec::new(),
            resume_contexts: Vec::new(),
            pending_discharges: Vec::new(),
            pending_sandbox_checks: Vec::new(),
            workspace_name: Arc::from(""),
            rigid_params: HashSet::new(),
        }
    }

    /// Seed the workspace package name user items are scoped under, so the
    /// checker mints [`ModuleId`](crate::fqn::ModuleId)s matching the
    /// registry's.
    pub(crate) fn set_workspace_name(&mut self, name: Arc<str>) {
        self.workspace_name = name;
    }

    /// Create a new inference context with standard abilities.
    #[must_use]
    pub fn new() -> Self {
        Self::with_parts(None, crate::ability_resolver::core_abilities())
    }

    /// Create a new inference context with an ability registry.
    #[must_use]
    pub fn with_registry(registry: AbilityRegistry) -> Self {
        Self::with_parts(Some(registry), crate::ability_resolver::core_abilities())
    }

    /// Create a new inference context with a custom ability resolver.
    #[must_use]
    pub fn with_resolver(resolver: AbilityResolver) -> Self {
        Self::with_parts(None, resolver)
    }

    /// Resolve every [`PendingDischarge`] recorded during body checking.
    ///
    /// Called after all bodies are checked (so ability variables that bind
    /// late are bound). Inner handles record before outer ones, and a
    /// callee's remainder may appear in any caller's body set, so this
    /// iterates to a fixpoint: each round resolves the discharges whose
    /// applied body no longer mentions another pending remainder. Anything
    /// left after a round without progress (mutual dependence or a
    /// genuinely polymorphic tail) resolves conservatively — handled
    /// abilities are subtracted from the concrete part and the tail is
    /// kept, i.e. an unknowable effect is assumed *unhandled*.
    pub(crate) fn resolve_pending_discharges(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_discharges);
        while !pending.is_empty() {
            let unresolved: Vec<AbilityVarId> = pending.iter().map(|p| p.remainder).collect();
            let (ready, blocked): (Vec<_>, Vec<_>) = pending.into_iter().partition(|p| {
                self.apply_abilities(&p.body)
                    .ability_var()
                    .is_none_or(|v| !unresolved.contains(&v))
            });
            let stuck = ready.is_empty();
            for p in &ready {
                self.bind_discharge(p);
            }
            if stuck {
                for p in &blocked {
                    self.bind_discharge(p);
                }
                break;
            }
            pending = blocked;
        }
    }

    /// Enforce every deferred sandbox restriction (see
    /// [`PendingSandboxCheck`]). Runs after
    /// [`Infer::resolve_pending_discharges`], so handle remainders inside
    /// sandbox bodies are already resolved. Violations land in
    /// `pending_errors`. A tail that is still free is genuinely
    /// polymorphic; only the concrete part can be judged.
    pub(crate) fn resolve_pending_sandbox_checks(&mut self) {
        let pending = std::mem::take(&mut self.pending_sandbox_checks);
        for check in pending {
            let applied = self.apply_abilities(&check.body);
            if let Some(err) = self.sandbox_violation(
                applied.concrete_abilities(),
                &check.allowed,
                &check.allowed_names,
                check.span,
            ) {
                self.pending_errors.push(err);
            }
        }
    }

    /// Bind one discharge's remainder variable to "body minus handled".
    fn bind_discharge(&mut self, discharge: &PendingDischarge) {
        let applied = self.apply_abilities(&discharge.body);
        let bound = match applied {
            AbilitySet::Concrete(ids) => AbilitySet::from_abilities(
                ids.into_iter().filter(|a| !discharge.handled.contains(a)),
            ),
            AbilitySet::Row { concrete, tail } => AbilitySet::row(
                concrete
                    .into_iter()
                    .filter(|a| !discharge.handled.contains(a)),
                tail,
            ),
            // Empty stays empty; a bare variable is unknowable — assume
            // nothing was handled. Unresolved never survives checking.
            other => other,
        };
        self.ability_subst.insert(discharge.remainder, bound);
    }

    /// Register a type alias under its bare name (a local or primitive
    /// type, resolvable by the bare name the Type IR carries).
    pub fn register_type_alias(&mut self, name: Arc<str>, ty: Type) {
        self.type_aliases.insert(NameKey::Bare(name), ty);
    }

    /// Register a type alias under a cross-module type's [`Fqn`] identity,
    /// the key a qualified constructor (`pkg::shapes::Money { … }`)
    /// resolves to.
    pub fn register_type_alias_item(&mut self, fqn: crate::fqn::Fqn, ty: Type) {
        self.type_aliases.insert(NameKey::Item(fqn), ty);
    }

    /// Look up a type alias by its bare name (Type IR names, local and
    /// primitive types).
    #[must_use]
    pub fn get_type_alias(&self, name: &str) -> Option<&Type> {
        self.type_aliases.get(&NameKey::Bare(Arc::from(name)))
    }

    /// Resolve a bare (unparameterized) named type to a concrete type: the
    /// registered type alias of that name, if any.
    ///
    /// The four primitives are no longer a context-independent shortcut here:
    /// they arrive as ordinary prelude imports (registered as aliases like any
    /// other type), so a bare `String` resolves only where the prelude — or an
    /// explicit `use` — put it in scope. A registry-less check therefore never
    /// resolves a primitive by name; the one path that needs them without
    /// imports (ability resolution) seeds them explicitly from the prelude via
    /// [`ModuleRegistry::prelude_type_aliases`]. This is the single point
    /// `resolve_holes` and unification consult, so an annotation `String` and a
    /// `String` literal always meet as the same nominal.
    #[must_use]
    pub(crate) fn expand_named_alias(&self, name: &str) -> Option<Type> {
        self.get_type_alias(name).cloned()
    }

    /// Look up a type alias by a reference's resolution key (a bare local
    /// type or a cross-module type's [`Fqn`]).
    #[must_use]
    pub fn get_type_alias_key(&self, key: &NameKey) -> Option<&Type> {
        self.type_aliases.get(key)
    }

    /// Drop every registered type alias whose key does not satisfy `keep`.
    ///
    /// Used to retract foreign package aliases that were registered only to
    /// hydrate imported signatures and impl targets, so they don't remain
    /// resolvable by bare name in this module's own bodies.
    pub(crate) fn retain_type_aliases(&mut self, keep: impl Fn(&NameKey) -> bool) {
        self.type_aliases.retain(|key, _| keep(key));
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        self.r#gen.fresh()
    }

    /// Generate a fresh ability variable.
    pub fn fresh_ability_var(&mut self) -> AbilitySet {
        self.r#gen.fresh_ability_var()
    }

    /// Add an ability to the current requirements, including its dependencies.
    pub fn require_ability(&mut self, ability: AbilityId) {
        // Add the ability and all its dependencies
        let abilities = if let Some(registry) = &self.ability_registry {
            registry.ability_with_dependencies(ability)
        } else {
            AbilitySet::single(ability)
        };
        self.current_abilities = self.current_abilities.union(&abilities);
    }

    /// Add an ability set to the current requirements.
    pub fn require_abilities(&mut self, abilities: &AbilitySet) {
        self.current_abilities = self.current_abilities.union(abilities);
    }

    /// Run `f` with a clean effect accumulator and return its result along
    /// with the effects it accumulated. The previous accumulator is
    /// restored either way; the caller decides where the inner effects
    /// flow (a lambda carries them on its type, a handle discharges some,
    /// a sandbox re-requires them after checking the restriction).
    pub(crate) fn with_isolated_effects<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> T,
    ) -> (T, AbilitySet) {
        let saved = std::mem::take(&mut self.current_abilities);
        let result = f(self);
        let inner = std::mem::replace(&mut self.current_abilities, saved);
        (result, inner)
    }

    /// Get the current ability requirements.
    #[must_use]
    pub fn current_abilities(&self) -> &AbilitySet {
        &self.current_abilities
    }

    /// Reset ability tracking (e.g., when entering a new function body).
    pub fn reset_abilities(&mut self) {
        self.current_abilities = AbilitySet::Empty;
    }

    /// Run `f` with `params` marked rigid, restoring the previous set after.
    ///
    /// Everything checked inside the closure — a function/method body and all
    /// its nested lambdas and `let`s — sees these names resolve to
    /// [`Type::Param`] through [`resolve_holes`](Self::resolve_holes). Nesting
    /// composes: an inner impl-method scope adds to (and then restores) the
    /// outer set. Only body checking calls this; signatures never do.
    pub(crate) fn with_rigid_params<T>(
        &mut self,
        params: impl IntoIterator<Item = Arc<str>>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = self.rigid_params.clone();
        self.rigid_params.extend(params);
        let result = f(self);
        self.rigid_params = saved;
        result
    }

    /// Resolve type holes (`_`) in a type annotation by replacing them with fresh
    /// type variables. This enables partial annotation where users can specify
    /// some parts of a type and let inference determine the rest.
    pub fn resolve_holes(&mut self, ty: &Type) -> Type {
        match ty {
            Type::Hole => self.fresh(),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| self.resolve_holes(e)).collect())
            }
            Type::Record(rec) => Type::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), self.resolve_holes(t)))
                    .collect(),
            )),
            Type::Function(f) => {
                let params = f.params.iter().map(|p| self.resolve_holes(p)).collect();
                let ret = self.resolve_holes(&f.ret);
                let abilities = self.resolve_ability_annotation(&f.abilities);
                Type::function_with_abilities(params, ret, abilities)
            }
            Type::Named(n) => {
                // A `Handler<A>` / `Handler<A, R>` annotation resolves to a
                // first-class handler type: `A` is an ability name (resolved
                // to its id under the same namespace policy every other
                // ability position obeys), `R` is the answer type (a fresh
                // var when omitted, so `Handler<A>` means "R inferred").
                if n.name.as_ref() == "Handler"
                    && matches!(n.args.len(), 1 | 2)
                    && let Type::Named(ability) = &n.args[0]
                    && let Some(ability_id) = self.resolve_annotated_ability(&ability.name)
                {
                    let answer = if n.args.len() == 2 {
                        self.resolve_holes(&n.args[1])
                    } else {
                        self.fresh()
                    };
                    return Type::handler(ability_id, answer);
                }

                // A bare name rigid in the current body is a type parameter,
                // not a nominal reference — mint a `Param`. Checked first so a
                // parameter named like a primitive/alias still stays rigid, and
                // so a leftover `Named` unambiguously means "unresolved
                // nominal" (what Phase 2's resolve-or-error keys on).
                if n.args.is_empty() && self.rigid_params.contains(&n.name) {
                    return Type::Param(Arc::clone(&n.name));
                }
                // A bare name that denotes a registered type alias or a
                // builtin primitive resolves to that type (see
                // `expand_named_alias`).
                if n.args.is_empty()
                    && let Some(expanded) = self.expand_named_alias(&n.name)
                {
                    return self.resolve_holes(&expanded);
                }
                let args = n.args.iter().map(|a| self.resolve_holes(a)).collect();
                // Attach an enum's nominal identity so an annotation like
                // `: Tree<number>` carries the same uuid the enum's
                // constructors and patterns produce. This covers declared
                // enums and the reserved-name prelude enums (`Option`/`Result`)
                // alike, since both are registered with a canonical uuid.
                if let Some(info) = self.enum_registry.get(&n.name) {
                    let uuid = info.uuid;
                    return Type::Named(NamedType::with_identity(Arc::clone(&n.name), args, uuid));
                }
                // Otherwise keep as named type, preserving any existing identity.
                Type::Named(n.map_args(args))
            }
            Type::Nominal(n) => Type::Nominal(n.map_inner(self.resolve_holes(&n.inner))),
            Type::AbilityValue(av) => Type::AbilityValue(AbilityValueType::new(
                self.resolve_holes(&av.result),
                self.resolve_ability_annotation(&av.ability),
            )),
            Type::Forall(f) => Type::Forall(ForallType::with_abilities(
                f.vars.clone(),
                f.ability_vars.clone(),
                self.resolve_holes(&f.body),
            )),
            // Other types remain unchanged
            _ => ty.clone(),
        }
    }

    /// Resolve ability names from a source annotation to concrete ability IDs.
    ///
    /// Lowering has no ability resolver, so annotations like
    /// `(T) -> U with core::system::Stdio` arrive as
    /// `AbilitySet::Unresolved(["core::system::Stdio"])` — qualified names
    /// keep their `::`-joined spelling so the namespace policy applies
    /// here exactly like every other position that names an ability.
    /// Errors are recorded in `pending_errors` (drained by the
    /// module-level check functions) rather than silently dropped.
    fn resolve_ability_annotation(&mut self, abilities: &AbilitySet) -> AbilitySet {
        let AbilitySet::Unresolved(names) = abilities else {
            return abilities.clone();
        };

        let ids = names
            .iter()
            .filter_map(|name| self.resolve_annotated_ability(name))
            .collect::<Vec<_>>();
        AbilitySet::from_abilities(ids)
    }

    /// Resolve one `::`-joined ability name from a source annotation to its
    /// id under the namespace policy — the same rule performs, `with`
    /// clauses, and handler arms enforce: a bare name names a local
    /// dynamic; a qualified one names its declaring module. Returns `None`
    /// and records a diagnostic in `pending_errors` on failure, so a bad
    /// annotation reports the real namespace error instead of silently
    /// resolving through a spelling-blind lookup.
    fn resolve_annotated_ability(&mut self, name: &str) -> Option<AbilityId> {
        let mut segments: Vec<&str> = name.split("::").collect();
        let bare = segments.pop().unwrap_or_default();
        let namespace = (!segments.is_empty())
            .then(|| crate::fqn::ModuleId::from_dotted_segments(&segments, &self.workspace_name));
        match self.ability_resolver.resolve_ref(namespace.as_ref(), bare) {
            Ok(id) => Some(id),
            Err(err) => {
                let kind = match err {
                    crate::ability_resolver::AbilityRefError::RequiresNamespace { namespace } => {
                        TypeErrorKind::AbilityRequiresNamespace {
                            ability: Arc::from(bare),
                            expected_namespace: namespace,
                        }
                    }
                    crate::ability_resolver::AbilityRefError::Unknown => {
                        TypeErrorKind::UnknownAbility {
                            name: Arc::from(name),
                        }
                    }
                };
                self.pending_errors
                    .push(Box::new(TypeError::new(kind, (0, 0))));
                None
            }
        }
    }

    /// Take any errors recorded outside the normal `InferResult` flow.
    pub(crate) fn take_pending_errors(&mut self) -> Vec<error::BoxedTypeError> {
        std::mem::take(&mut self.pending_errors)
    }

    /// Instantiate a type scheme with fresh type variables.
    pub fn instantiate(&mut self, scheme: &Scheme) -> Type {
        if scheme.vars.is_empty() && scheme.ability_vars.is_empty() {
            return scheme.ty.clone();
        }

        let mut type_subst = HashMap::new();
        for var in &scheme.vars {
            type_subst.insert(*var, self.fresh());
        }

        let mut ability_subst = HashMap::new();
        for var in &scheme.ability_vars {
            ability_subst.insert(*var, self.fresh_ability_var());
        }

        scheme.ty.substitute_all(&type_subst, &ability_subst)
    }

    /// Generalize a type to a scheme by quantifying free variables
    /// not in the environment.
    ///
    /// The environment's free variables are computed *after applying the
    /// current substitution*: a stored `'3` that unification later bound to
    /// `('7) -> ()` pins `'7` in the environment even though the raw stored
    /// type never mentions it. Skipping this application would wrongly
    /// quantify `'7` here and let one binding's type vary per use site.
    #[must_use]
    pub fn generalize(&self, env: &TypeEnv, ty: &Type) -> Scheme {
        let ty = self.apply(ty);
        let ty_vars = ty.free_vars();

        let mut env_vars = Vec::new();
        let mut env_ability_vars = Vec::new();
        for (_, scheme) in env.iter() {
            // Quantified variables never enter the substitution, so applying
            // it leaves them intact; they are excluded as bound below.
            let applied = self.apply(&scheme.ty);
            env_vars.extend(
                applied
                    .free_vars()
                    .into_iter()
                    .filter(|v| !scheme.vars.contains(v)),
            );
            env_ability_vars.extend(
                applied
                    .free_ability_vars()
                    .into_iter()
                    .filter(|v| !scheme.ability_vars.contains(v)),
            );
        }

        let free_type_vars: Vec<_> = ty_vars
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();

        let ty_ability_vars = ty.free_ability_vars();

        let free_ability_vars: Vec<_> = ty_ability_vars
            .into_iter()
            .filter(|v| !env_ability_vars.contains(v))
            .collect();

        if free_type_vars.is_empty() && free_ability_vars.is_empty() {
            Scheme::mono(ty)
        } else {
            Scheme::poly_with_abilities(free_type_vars, free_ability_vars, ty)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AbilityId;

    /// A distinct, recognizable `AbilityId` for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    #[test]
    fn test_type_error_display() {
        let err = TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: Type::number(),
                actual: Type::string(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("type mismatch"));
        assert!(msg.contains("Number"));
        assert!(msg.contains("String"));
    }

    #[test]
    fn test_ability_error_display() {
        let err = TypeError::new(
            TypeErrorKind::AbilityMismatch {
                expected: AbilitySet::from_abilities([aid(1)]),
                actual: AbilitySet::from_abilities([aid(2)]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("ability mismatch"));

        let err2 = TypeError::new(
            TypeErrorKind::UnknownAbility { name: "Foo".into() },
            (0, 10),
        );
        let msg2 = format!("{err2}");
        assert!(msg2.contains("unknown ability"));
        assert!(msg2.contains("Foo"));
    }

    #[test]
    fn test_error_display_field_not_found() {
        let err = TypeError::new(
            TypeErrorKind::FieldNotFound {
                field: "missing".into(),
                record_ty: Type::record([("x", Type::number())]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("missing") || msg.contains("field"));
    }

    #[test]
    fn test_error_display_tuple_index_out_of_bounds() {
        let err = TypeError::new(
            TypeErrorKind::TupleIndexOutOfBounds {
                index: 5,
                tuple_ty: Type::Tuple(vec![Type::number(), Type::string()]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("5") || msg.contains("out of bounds") || msg.contains("index"));
    }

    #[test]
    fn test_error_display_not_a_function() {
        let err = TypeError::new(TypeErrorKind::NotAFunction { ty: Type::number() }, (0, 10));
        let msg = format!("{err}");
        assert!(msg.contains("not a function") || msg.contains("Number"));
    }

    #[test]
    fn test_error_display_non_boolean_condition() {
        let err = TypeError::new(
            TypeErrorKind::NonBooleanCondition { ty: Type::number() },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("condition") || msg.contains("Bool"));
    }

    #[test]
    fn test_error_display_arity_mismatch() {
        let err = TypeError::new(
            TypeErrorKind::ArityMismatch {
                expected: 2,
                actual: 1,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("2") && msg.contains("1"));
    }

    #[test]
    fn test_error_display_match_arm_type_mismatch() {
        let err = TypeError::new(
            TypeErrorKind::MatchArmTypeMismatch {
                first: Type::number(),
                arm: Type::string(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("match") || msg.contains("arm"));
    }

    #[test]
    fn test_error_display_undefined_variable() {
        let err = TypeError::new(
            TypeErrorKind::UndefinedVariable { name: "foo".into() },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("foo") || msg.contains("undefined"));
    }

    #[test]
    fn test_error_display_missing_ability() {
        let err = TypeError::new(
            TypeErrorKind::MissingAbility {
                required: aid(1),
                available: AbilitySet::Empty,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("ability") || msg.contains("missing") || msg.contains("require"));
    }

    #[test]
    fn test_error_display_sandbox_ability_violation() {
        let err = TypeError::new(
            TypeErrorKind::SandboxAbilityViolation {
                ability: "FileSystem".into(),
                allowed: vec!["Console".into()],
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("sandbox") || msg.contains("FileSystem") || msg.contains("not allowed")
        );
    }

    #[test]
    fn test_error_display_handler_missing_method() {
        let err = TypeError::new(
            TypeErrorKind::HandlerMissingMethod {
                ability: "Console".into(),
                method: "print".into(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("print") || msg.contains("missing") || msg.contains("Console"));
    }

    #[test]
    fn test_error_display_infinite_type() {
        let err = TypeError::new(
            TypeErrorKind::InfiniteType {
                var: 0,
                ty: Type::function(vec![Type::var(0)], Type::var(0)),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("infinite") || msg.contains("recursive") || msg.contains("occurs"));
    }

    #[test]
    fn test_error_display_cannot_infer() {
        let err = TypeError::new(
            TypeErrorKind::CannotInfer {
                hint: "ambiguous record field access".into(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("cannot") || msg.contains("infer") || msg.contains("ambiguous"));
    }
}
