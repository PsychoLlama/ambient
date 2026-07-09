//! Ability type inference and lookup.
//!
//! This module handles:
//! - Ability name/ID conversion
//! - Method signature lookup
//! - Ability tracking during inference

use std::sync::Arc;

use super::{Infer, InferResult, TypeErrorKind, type_error};
use crate::ast::QualifiedName;
use crate::types::{AbilityId, AbilitySet, Type};

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

    /// The full declared signature of an ability method, instantiated for
    /// one use site: quantified type parameters of dynamic methods become
    /// fresh inference variables, and builtin-descriptor type variables
    /// (which arrive as `Hole`) resolve to fresh variables too.
    ///
    /// This is the one lookup handler arms, handler literals, and perform
    /// checking share, so all three enforce the same signature.
    pub(crate) fn ability_method_signature(
        &mut self,
        ability_id: AbilityId,
        method_name: &str,
    ) -> Option<(Vec<Type>, Type)> {
        // Every ability is a module-declared (dynamic) ability carrying
        // fully resolved types.
        let dynamic = self
            .ability_resolver
            .get_dynamic_by_id(ability_id)
            .cloned()?;
        let method = dynamic.method(method_name)?;
        let mut subst = std::collections::HashMap::new();
        for quantified in &method.quantified {
            subst.insert(*quantified, self.fresh());
        }
        // `resolve_holes` re-attaches reserved enum identities from the
        // checking context (see `lookup_dynamic_method`): the dynamic
        // method's types were resolved without an enum registry, so
        // prelude `Option`/`Result` arrive uuid-less and would miss
        // inherent-method dispatch and unification against uuid-bearing
        // values.
        let params = method
            .params
            .iter()
            .map(|p| self.resolve_holes(&p.substitute(&subst)))
            .collect();
        let ret = self.resolve_holes(&method.ret.substitute(&subst));
        Some((params, ret))
    }

    /// Look up an ability method and return its ID, result type, and
    /// additional abilities to require (the ability's declared dependencies).
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if the ability or method is not found, or if the namespace is incorrect.
    pub fn lookup_ability_method(
        &mut self,
        ability: &QualifiedName,
        method_name: &str,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(AbilityId, Type, AbilitySet)> {
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
        self.lookup_dynamic_method(&dynamic, method_name, arg_tys, span)
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

        if method.params.len() != arg_tys.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: method.params.len(),
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        let mut subst = std::collections::HashMap::new();
        for quantified in &method.quantified {
            subst.insert(*quantified, self.fresh());
        }

        for (param, arg) in method.params.iter().zip(arg_tys) {
            // `resolve_holes` re-attaches reserved enum identities
            // (`Option`/`Result` uuids) from the *checking* context: the
            // dynamic method's types were resolved in isolation (the
            // ability prelude has no enum registry), so a declared
            // `Option<T>` param arrives as a bare `Named("Option")` and
            // would fail to unify against an argument that carries the
            // uuid its constructors produce.
            let param = self.resolve_holes(&param.substitute(&subst));
            self.unify(&param, arg, span)?;
        }

        // Likewise normalize the result type so a method returning
        // `Option<T>`/`Result<T, E>` dispatches its inherent methods
        // (`.unwrap_or`, ...) — those key on the reserved enum uuid, which
        // only the checking context supplies.
        let ret = self.resolve_holes(&method.ret.substitute(&subst));
        let ret = self.apply(&ret);
        let additional = AbilitySet::from_abilities(dynamic.dependencies.iter().copied());
        Ok((dynamic.id, ret, additional))
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
            name: Arc::from("Printer"),
            methods: vec![crate::ability_resolver::DynMethod {
                id: 0,
                name: Arc::from("go"),
                param_names: vec![],
                params: vec![Type::string()],
                ret: Type::Unit,
                quantified: vec![],
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
            .lookup_ability_method(&qualified, "go", &[Type::string()], span())
            .expect("qualified perform should resolve");
        assert_eq!(id, aid(7));
        assert_eq!(ret, Type::Unit);

        // Declared signatures are enforced: wrong argument type fails.
        let err = infer.lookup_ability_method(&qualified, "go", &[Type::number()], span());
        assert!(err.is_err(), "argument type mismatch should be rejected");

        // The wrong namespace does not resolve.
        let wrong = QualifiedName::qualified(vec!["other"], "Printer");
        assert!(
            infer
                .lookup_ability_method(&wrong, "go", &[Type::string()], span())
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
        let result = infer.lookup_ability_method(&bare, "go", &[Type::string()], span());
        assert!(
            result.is_err(),
            "Printer without platform. prefix should fail"
        );

        // The same perform with the namespace succeeds.
        let qualified = platform_ability("Printer");
        let result = infer.lookup_ability_method(&qualified, "go", &[Type::string()], span());
        assert!(result.is_ok(), "core::system::Printer::go should succeed");
    }
}
