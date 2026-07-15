//! Ability type inference and lookup.
//!
//! This module handles:
//! - Ability name/ID conversion
//! - Method signature lookup
//! - Ability tracking during inference

use std::collections::HashMap;
use std::sync::Arc;

use super::error::TypeError;
use super::{Infer, InferResult, TypeErrorKind, type_error};
use crate::ast::QualifiedName;
use crate::types::{AbilityId, AbilitySet, AbilityVarId, Type};

impl Infer {
    // ─────────────────────────────────────────────────────────────────────────
    // Ability lookup helpers (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────

    /// Convert an ability name to its ID using the resolver.
    ///
    /// Low-level bare lookup for rendering and set comparisons. Source
    /// positions that name abilities (performs, `with` clauses, effect
    /// rows, handler arms, sandbox clauses) must resolve through
    /// [`Self::resolve_ability_ref`], which enforces the namespace policy.
    pub(crate) fn ability_name_to_id(&self, name: &str) -> Option<AbilityId> {
        self.ability_resolver.name_to_id(name)
    }

    /// The namespace [`ModuleId`](crate::fqn::ModuleId) an ability reference
    /// names, or `None` for a bare reference (a local or builtin ability).
    ///
    /// A resolved reference names its canonicalized declaring module; an
    /// unresolved but path-qualified one (e.g. a platform declaration's
    /// `with core::system::Stdio`, which never runs the resolve pass) falls
    /// back to its spelled path, scoped under the workspace.
    pub(crate) fn ability_namespace(
        &self,
        ability: &QualifiedName,
    ) -> Option<crate::fqn::ModuleId> {
        if let Some(fqn) = &ability.resolved {
            return Some(fqn.module.clone());
        }
        if ability.path.is_empty() {
            return None;
        }
        let segments: Vec<&str> = ability.path.iter().map(AsRef::as_ref).collect();
        Some(crate::fqn::ModuleId::from_dotted_segments(
            &segments,
            &self.workspace_name,
        ))
    }

    /// Resolve an ability reference as written in source, enforcing the
    /// namespace policy: namespaced (platform) abilities must be written
    /// with their prefix everywhere, locals and builtins bare.
    ///
    /// # Errors
    ///
    /// `AbilityRequiresNamespace` when a namespaced ability was named
    /// bare (or under the wrong namespace); `UnknownAbility` otherwise.
    /// The error span prefers the reference's own name span when carried.
    pub(crate) fn resolve_ability_ref(
        &self,
        ability: &QualifiedName,
        fallback_span: (u32, u32),
    ) -> InferResult<AbilityId> {
        let span = ability
            .name_span
            .map_or(fallback_span, |s| (s.start, s.end));
        let namespace = self.ability_namespace(ability);
        self.ability_resolver
            .resolve_ref(namespace.as_ref(), ability.resolved_name())
            .map_err(|err| {
                let kind = match err {
                    crate::ability_resolver::AbilityRefError::RequiresNamespace { namespace } => {
                        TypeErrorKind::AbilityRequiresNamespace {
                            ability: Arc::clone(&ability.name),
                            expected_namespace: namespace,
                        }
                    }
                    crate::ability_resolver::AbilityRefError::Unknown => {
                        TypeErrorKind::UnknownAbility {
                            name: Arc::clone(&ability.name),
                        }
                    }
                };
                type_error(kind, span)
            })
    }

    /// Convert an ability ID to its name using the resolver.
    pub(crate) fn ability_id_to_name(&self, id: AbilityId) -> Option<&str> {
        self.ability_resolver.id_to_name(id)
    }

    /// The signature and bound context a *handler arm* covering
    /// `method_name` checks against: the declared parameter and return
    /// types with the method's type parameters made **rigid**
    /// ([`Type::Param`]) rather than instantiated, the rigid parameter
    /// names, and the resolved `(param, trait)` bounds in dictionary order.
    ///
    /// A perform instantiates the method's type parameters to fresh
    /// variables (a call at a concrete type); a handler arm is the opposite
    /// — it must work for *every* instantiation the delimited body performs,
    /// so its type parameters are rigid, exactly like an ordinary function
    /// body's. The arm then receives one dictionary per bound (the perform
    /// pushes them and the compiler binds them to the arm's `<dict#N>`
    /// pseudo-locals), so `x.eq(y)` inside the arm dispatches through the
    /// delivered dictionary. Returns `None` if the ability or method is
    /// unknown.
    #[allow(clippy::type_complexity)]
    pub(crate) fn ability_method_arm_signature(
        &mut self,
        ability_id: AbilityId,
        method_name: &str,
    ) -> Option<(
        Vec<Type>,
        Type,
        Vec<Arc<str>>,
        Vec<(Arc<str>, crate::types::TraitBound)>,
    )> {
        let dynamic = self
            .ability_resolver
            .get_dynamic_by_id(ability_id)
            .cloned()?;
        let method = dynamic.method(method_name)?;

        // Each type parameter is rigid in the arm: map its quantified
        // variable to `Type::Param(name)` (parallel lists, ability/row
        // variables excluded from both).
        let mut subst = std::collections::HashMap::new();
        for (var, name) in method.quantified.iter().zip(&method.type_param_names) {
            subst.insert(*var, Type::Param(Arc::clone(name)));
        }
        // Ability (row) variables still freshen per arm, like a perform.
        let mut ability_subst = std::collections::HashMap::new();
        for quantified in &method.quantified_abilities {
            ability_subst.insert(*quantified, self.fresh_ability_var());
        }

        let params = method
            .params
            .iter()
            .map(|p| self.resolve_holes(&p.substitute_all(&subst, &ability_subst)))
            .collect();
        let ret = self.resolve_holes(&method.ret.substitute_all(&subst, &ability_subst));

        // Resolve the method's bounds into the arm's dictionary-parameter
        // context, in dictionary order (the `bounds` list already is). Each
        // bound carries the resolve pass's canonical `Fqn`, so it resolves to
        // the defining module's trait regardless of this module's scope — an
        // unknown one is already reported at every perform site, so dropping
        // it here only skews indices in a module that will not compile.
        let mut bounds = Vec::with_capacity(method.bounds.len());
        for (idx, bound) in &method.bounds {
            let Some(name) = method.type_param_names.get(*idx) else {
                continue;
            };
            // Unknown traits were already reported at every perform site;
            // dropping here only skews indices in a module that won't compile.
            let Some(resolved) = self.resolve_trait_ref(bound, (0, 0), &mut Vec::new()) else {
                continue;
            };
            bounds.push((Arc::clone(name), resolved));
        }

        Some((params, ret, method.type_param_names.clone(), bounds))
    }

    /// Look up an ability method and return its ID, result type, and
    /// additional abilities to require (the ability's declared dependencies).
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if the ability or method is not found, or if the namespace is incorrect.
    pub fn lookup_ability_method(
        &mut self,
        ability: Option<&QualifiedName>,
        method_name: &str,
        arg_tys: &[Type],
        dicts: &mut Option<crate::ast::Dicts>,
        fingerprints: &mut Option<crate::ast::Fingerprints>,
        span: (u32, u32),
    ) -> InferResult<(AbilityId, Type, AbilitySet)> {
        // A bare-method perform (`seed!(…)`) resolves through an imported
        // ability method; the resolve pass fills `ability` when one is in
        // scope. Still-`None` means no import covers the name — diagnose,
        // suggesting a `use` when some registered ability declares it.
        let Some(ability) = ability else {
            return Err(type_error(
                TypeErrorKind::UnimportedAbilityMethod {
                    method: method_name.into(),
                    suggestion: self.ability_resolver.suggest_method_import(method_name),
                },
                span,
            ));
        };
        // One policy for every position that names an ability: namespaced
        // dynamics (ability preludes, e.g. the `platform` module, and the
        // prelude-injected `Exception`) resolve only under their declaring
        // module's namespace, local declarations bare, with locals shadowing
        // (mirroring how local enums shadow the prelude).
        let ability_id = self.resolve_ability_ref(ability, span)?;

        // Every resolved ability is a dynamic carrying its full declared
        // signature; arguments unify against the declared parameter types
        // (so a perform like `Exception::throw!(42)` must not type-check).
        let dynamic = self
            .ability_resolver
            .get_dynamic_by_id(ability_id)
            .cloned()
            .ok_or_else(|| {
                type_error(
                    TypeErrorKind::UnknownAbilityMethod {
                        ability: Arc::clone(&ability.name),
                        method: method_name.into(),
                    },
                    span,
                )
            })?;
        self.lookup_dynamic_method(&dynamic, method_name, arg_tys, dicts, fingerprints, span)
    }

    /// Type-check a call to a module-declared ability method.
    ///
    /// Unlike builtin descriptors (which only expose a return type),
    /// dynamic abilities carry full declared signatures, so arguments are
    /// unified against the declared parameter types. Quantified method
    /// type parameters instantiate to fresh variables per call site. The
    /// returned ability set carries the ability's declared dependencies so
    /// performing it also requires them.
    fn lookup_dynamic_method(
        &mut self,
        dynamic: &crate::ability_resolver::DynAbility,
        method_name: &str,
        arg_tys: &[Type],
        dicts: &mut Option<crate::ast::Dicts>,
        fingerprints: &mut Option<crate::ast::Fingerprints>,
        span: (u32, u32),
    ) -> InferResult<(AbilityId, Type, AbilitySet)> {
        let Some(method) = dynamic.method(method_name).cloned() else {
            return Err(type_error(
                TypeErrorKind::UnknownAbilityMethod {
                    ability: Arc::clone(&dynamic.name),
                    method: method_name.into(),
                },
                span,
            ));
        };

        // The State ability's write-path methods declare trailing
        // fingerprint parameters that perform sites never spell — the
        // compiler supplies them (see `super::fingerprints`). Recognized
        // by the reserved uuid, never by name.
        let hidden = if dynamic.uuid == ambient_core::state::STATE_UUID {
            super::fingerprints::hidden_fingerprint_params(method_name)
        } else {
            0
        };

        let expected = method.params.len().saturating_sub(hidden);
        if expected != arg_tys.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected,
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        let mut subst = std::collections::HashMap::new();
        for quantified in &method.quantified {
            subst.insert(*quantified, self.fresh());
        }
        // Ability (row) variables freshen per perform site: an effectful
        // lambda argument binds the fresh row, but that row stays local to
        // this call — it does not join the caller's required abilities just
        // from being passed (the perform requires the ability and its
        // declared dependencies, nothing more).
        let mut ability_subst = std::collections::HashMap::new();
        for quantified in &method.quantified_abilities {
            ability_subst.insert(*quantified, self.fresh_ability_var());
        }

        // Fingerprinted methods record a pending fingerprint group on the
        // perform, naming the instantiated cell types to render. The
        // method's `make`/`migrate`/`f` parameters are real function types,
        // so the cell type is solved by the ordinary argument unification
        // below — this only registers what to render.
        if hidden != 0 {
            self.record_state_fingerprints(&method, &subst, fingerprints, span)?;
        }

        // A bounded method records its dictionary constraints against the
        // perform expression. Each bound carries the resolve pass's canonical
        // `Fqn`, so it resolves to the defining module's trait here.
        if !method.bounds.is_empty() {
            let mut resolved_bounds = Vec::with_capacity(method.bounds.len());
            for (param_idx, bound) in &method.bounds {
                let mut errors = Vec::new();
                let Some(resolved) = self.resolve_trait_ref(bound, span, &mut errors) else {
                    return Err(errors.pop().unwrap_or_else(|| {
                        type_error(
                            TypeErrorKind::UnknownTrait {
                                name: Arc::clone(&bound.name.name),
                            },
                            span,
                        )
                    }));
                };
                let Some(&var) = method.quantified.get(*param_idx) else {
                    continue;
                };
                resolved_bounds.push((var, resolved));
            }
            *dicts = Some(self.record_bound_constraints(&resolved_bounds, &subst, span));
        }

        for (param, arg) in method.params.iter().zip(arg_tys) {
            // `resolve_holes` re-attaches reserved enum identities
            // (`Option`/`Result` uuids) from the *checking* context: the
            // dynamic method's types were resolved in isolation (the
            // ability prelude has no enum registry), so a declared
            // `Option<T>` param arrives as a bare `Named("Option")` and
            // would fail to unify against an argument that carries the
            // uuid its constructors produce.
            let param = self.resolve_holes(&param.substitute_all(&subst, &ability_subst));
            self.unify(&param, arg, span)?;
        }

        // Likewise normalize the result type so a method returning
        // `Option<T>`/`Result<T, E>` dispatches its inherent methods
        // (`.unwrap_or`, ...) — those key on the reserved enum uuid, which
        // only the checking context supplies.
        let ret = self.resolve_holes(&method.ret.substitute_all(&subst, &ability_subst));
        let ret = self.apply(&ret);
        let additional = AbilitySet::from_abilities(dynamic.dependencies.iter().copied());
        Ok((dynamic.id, ret, additional))
    }

    /// Run `f` with an item's ability (row) variables in scope.
    ///
    /// `report_type_misuse` controls whether a bare ability-variable name in
    /// a type position is reported: true while checking a body, false while
    /// building a scheme, so the same misuse reports exactly once. The
    /// previous scope is restored afterward.
    pub(super) fn with_ability_var_scope<T>(
        &mut self,
        scope: HashMap<Arc<str>, AbilityVarId>,
        report_type_misuse: bool,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved_scope = std::mem::replace(&mut self.ability_var_scope, scope);
        let saved_report =
            std::mem::replace(&mut self.report_ability_var_type_errors, report_type_misuse);
        let result = f(self);
        self.ability_var_scope = saved_scope;
        self.report_ability_var_type_errors = saved_report;
        result
    }

    /// Resolve ability names from a source annotation to an [`AbilitySet`].
    ///
    /// Lowering has no ability resolver, so annotations like
    /// `(T) -> U with core::system::Stdio` arrive as
    /// `AbilitySet::Unresolved(["core::system::Stdio"])` — qualified names
    /// keep their `::`-joined spelling so the namespace policy applies here
    /// exactly like every other position that names an ability. A bare name
    /// naming a declared ability (row) variable becomes the row's
    /// polymorphic tail (at most one per row). Errors are recorded in
    /// `pending_errors` rather than silently dropped.
    pub(super) fn resolve_ability_annotation(&mut self, abilities: &AbilitySet) -> AbilitySet {
        let AbilitySet::Unresolved(names) = abilities else {
            return abilities.clone();
        };

        let mut ids = Vec::new();
        let mut tail: Option<(Arc<str>, AbilityVarId)> = None;
        for name in names {
            if !name.contains("::")
                && let Some(&var) = self.ability_var_scope.get(name.as_ref())
            {
                match &tail {
                    // Reported only while checking a body, so a function-type
                    // annotation resolved once for the scheme and again for
                    // the body reports this exactly once.
                    Some((first, existing))
                        if *existing != var && self.report_ability_var_type_errors =>
                    {
                        self.pending_errors.push(Box::new(TypeError::new(
                            TypeErrorKind::MultipleRowVariables {
                                first: Arc::clone(first),
                                second: Arc::clone(name),
                            },
                            (0, 0),
                        )));
                    }
                    Some((_, existing)) if *existing != var => {}
                    _ => tail = Some((Arc::clone(name), var)),
                }
                continue;
            }
            if let Some(id) = self.resolve_annotated_ability(name) {
                ids.push(id);
            }
        }
        match tail {
            // `row` collapses to a bare `Var` when there are no concrete ids.
            Some((_, var)) => AbilitySet::row(ids, var),
            None => AbilitySet::from_abilities(ids),
        }
    }

    /// Resolve one `::`-joined ability name from a source annotation to its
    /// id under the namespace policy — the same rule performs, `with`
    /// clauses, and handler arms enforce: a bare name names a local
    /// dynamic; a qualified one names its declaring module. Returns `None`
    /// and records a diagnostic in `pending_errors` on failure, so a bad
    /// annotation reports the real namespace error instead of silently
    /// resolving through a spelling-blind lookup.
    pub(super) fn resolve_annotated_ability(&mut self, name: &str) -> Option<AbilityId> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::QualifiedName;
    use crate::infer::Infer;
    use crate::types::{AbilityId, AbilityInfo, AbilityRegistry, AbilitySet, Type};

    /// A distinct, recognizable `AbilityId` for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    fn span() -> (u32, u32) {
        (0, 0)
    }

    /// Create a qualified name with the `platform.` prefix.
    fn platform_ability(name: &str) -> QualifiedName {
        QualifiedName::qualified(vec!["core", "system"], name)
    }

    /// A prelude-style test ability: `Printer.go(message: string): ()`.
    fn printer_ability(byte: u8) -> crate::ability_resolver::DynAbility {
        crate::ability_resolver::DynAbility {
            id: aid(byte),
            uuid: uuid::Uuid::from_u128(u128::from(byte)),
            name: Arc::from("Printer"),
            methods: vec![crate::ability_resolver::DynMethod {
                name: Arc::from("go"),
                param_names: vec![],
                params: vec![Type::string()],
                ret: Type::Unit,
                quantified: vec![],
                type_param_names: vec![],
                quantified_abilities: vec![],
                bounds: Vec::new(),
                signature: ambient_core::SignatureHash::new(&["string"], "unit"),
                has_impl: true,
            }],
            dependencies: vec![],
        }
    }

    /// Namespaced dynamics (ability preludes) resolve qualified performs
    /// with full argument checking, superseding the descriptor path.
    #[test]
    fn namespaced_dynamic_resolves_qualified_perform() {
        let mut infer = Infer::new();
        infer.ability_resolver.register_dynamic_in_namespace(
            &crate::fqn::ModuleId::core_system(),
            printer_ability(7),
        );

        let qualified = QualifiedName::qualified(vec!["core", "system"], "Printer");
        let (id, ret, _) = infer
            .lookup_ability_method(
                Some(&qualified),
                "go",
                &[Type::string()],
                &mut None,
                &mut None,
                span(),
            )
            .expect("qualified perform should resolve");
        assert_eq!(id, aid(7));
        assert_eq!(ret, Type::Unit);

        // Declared signatures are enforced: wrong argument type fails.
        let err = infer.lookup_ability_method(
            Some(&qualified),
            "go",
            &[Type::number()],
            &mut None,
            &mut None,
            span(),
        );
        assert!(err.is_err(), "argument type mismatch should be rejected");

        // The wrong namespace does not resolve.
        let wrong = QualifiedName::qualified(vec!["other"], "Printer");
        assert!(
            infer
                .lookup_ability_method(
                    Some(&wrong),
                    "go",
                    &[Type::string()],
                    &mut None,
                    &mut None,
                    span()
                )
                .is_err()
        );
    }

    #[test]
    fn test_ability_tracking() {
        let mut infer = Infer::new();

        // Start with empty abilities
        assert!(infer.current_abilities().is_pure());

        // Require an ability
        infer.require_ability(aid(1));
        assert!(infer.current_abilities().contains(aid(1)));

        // Require another ability
        infer.require_ability(aid(2));
        assert!(infer.current_abilities().contains(aid(1)));
        assert!(infer.current_abilities().contains(aid(2)));

        // Reset
        infer.reset_abilities();
        assert!(infer.current_abilities().is_pure());
    }

    #[test]
    fn test_fresh_ability_var() {
        let mut infer = Infer::new();
        let v1 = infer.fresh_ability_var();
        let v2 = infer.fresh_ability_var();

        assert!(matches!(v1, AbilitySet::Var(0)));
        assert!(matches!(v2, AbilitySet::Var(1)));
    }

    #[test]
    fn test_resolve_ability_ref_policy() {
        let mut infer = Infer::new();
        infer.ability_resolver.register_dynamic_in_namespace(
            &crate::fqn::ModuleId::core_system(),
            printer_ability(7),
        );

        // There are no engine builtins: an ability the resolver has never
        // seen (registry-less, no prelude) is unknown, bare or qualified.
        // `Exception` is a namespaced dynamic (`core::exception`) in a real
        // check; its bare-yet-namespaced resolution is covered end-to-end by
        // the CLI/integration tests.
        assert!(
            infer
                .resolve_ability_ref(&QualifiedName::simple("Exception"), span())
                .is_err()
        );

        // Namespaced prelude abilities resolve only with their prefix —
        // in every position, not just performs.
        assert_eq!(
            infer
                .resolve_ability_ref(&platform_ability("Printer"), span())
                .ok(),
            Some(aid(7))
        );
        let bare = infer.resolve_ability_ref(&QualifiedName::simple("Printer"), span());
        assert!(matches!(
            bare.unwrap_err().kind,
            TypeErrorKind::AbilityRequiresNamespace { .. }
        ));

        // Unknown names are unknown.
        assert!(
            infer
                .resolve_ability_ref(&QualifiedName::simple("Unknown"), span())
                .is_err()
        );

        // The low-level bare lookup still resolves namespaced dynamics
        // (rendering/tooling only).
        assert_eq!(infer.ability_name_to_id("Printer"), Some(aid(7)));
    }

    #[test]
    fn test_require_ability_with_registry() {
        let mut registry = AbilityRegistry::new();

        // IO is ability 1
        registry.register(aid(1), AbilityInfo::new("IO"));

        // FileSystem (2) depends on IO (1)
        registry.register(
            aid(2),
            AbilityInfo::new("FileSystem").with_dependency(aid(1)),
        );

        let mut infer = Infer::with_registry(registry);

        // When we require FileSystem, IO should also be required
        infer.require_ability(aid(2));

        let abilities = infer.current_abilities();
        if let AbilitySet::Concrete(ids) = abilities {
            assert!(ids.contains(&aid(1)), "IO should be required");
            assert!(ids.contains(&aid(2)), "FileSystem should be required");
        } else {
            panic!("Expected concrete ability set");
        }
    }

    #[test]
    fn test_platform_namespace_required() {
        let mut infer = Infer::new();
        infer.ability_resolver.register_dynamic_in_namespace(
            &crate::fqn::ModuleId::core_system(),
            printer_ability(7),
        );

        // A namespaced prelude ability performed bare should fail.
        let bare = QualifiedName::simple("Printer");
        let result = infer.lookup_ability_method(
            Some(&bare),
            "go",
            &[Type::string()],
            &mut None,
            &mut None,
            span(),
        );
        assert!(
            result.is_err(),
            "Printer without platform. prefix should fail"
        );

        // The same perform with the namespace succeeds.
        let qualified = platform_ability("Printer");
        let result = infer.lookup_ability_method(
            Some(&qualified),
            "go",
            &[Type::string()],
            &mut None,
            &mut None,
            span(),
        );
        assert!(result.is_ok(), "core::system::Printer::go should succeed");
    }
}
