//! Ability type inference and lookup.
//!
//! This module handles:
//! - Ability name/ID conversion
//! - Method signature lookup
//! - Ability tracking during inference

use std::sync::Arc;

use super::{Infer, InferResult, TypeErrorKind, type_error};
use crate::ability_resolver::EngineTypeFactory;
use crate::ast::QualifiedName;
use crate::types::{AbilityId, AbilitySet, Type};

impl Infer {
    // ─────────────────────────────────────────────────────────────────────────
    // Ability lookup helpers (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────

    /// Convert an ability name to its ID using the resolver.
    pub(crate) fn ability_name_to_id(&self, name: &str) -> Option<AbilityId> {
        self.ability_resolver.name_to_id(name)
    }

    /// Convert an ability ID to its name using the resolver.
    pub(crate) fn ability_id_to_name(&self, id: AbilityId) -> Option<&str> {
        self.ability_resolver.id_to_name(id)
    }

    /// Try to infer which ability a handler literal is for based on method names.
    ///
    /// Returns the ability ID if all methods belong to exactly one ability.
    pub(crate) fn infer_ability_from_methods(
        &self,
        method_names: &[Arc<str>],
    ) -> Option<AbilityId> {
        self.ability_resolver
            .infer_ability_from_methods(method_names)
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
        // Module-declared (dynamic) abilities carry fully resolved types.
        if let Some(dynamic) = self.ability_resolver.get_dynamic_by_id(ability_id).cloned() {
            let method = dynamic.method(method_name)?;
            let mut subst = std::collections::HashMap::new();
            for quantified in &method.quantified {
                subst.insert(*quantified, self.fresh());
            }
            let params = method.params.iter().map(|p| p.substitute(&subst)).collect();
            let ret = method.ret.substitute(&subst);
            return Some((params, ret));
        }

        // Builtin descriptors construct types through the factory.
        let factory = EngineTypeFactory;
        let (params, ret) = {
            let ability = self.ability_resolver.get_by_id(ability_id)?;
            let method = ability.get_method(method_name)?;
            (
                (method.signature.param_types)(&factory),
                (method.signature.return_type)(&factory),
            )
        };
        let params = params.iter().map(|p| self.resolve_holes(p)).collect();
        let ret = self.resolve_holes(&ret);
        Some((params, ret))
    }

    /// Look up an ability method and return its ID, result type, and additional abilities to require.
    ///
    /// For builtin abilities, the additional abilities set is empty. For module-declared
    /// abilities, it carries the ability's declared dependencies.
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
        let ability_name = &ability.name;

        // Namespaced dynamic abilities (ability preludes, e.g. the
        // `platform` module) resolve qualified performs with full declared
        // signatures. They take precedence over the builtin-descriptor
        // path below so a prelude declaration supersedes any descriptor
        // registered under the same name.
        if ability.path.len() == 1
            && let Some(dynamic) = self
                .ability_resolver
                .get_namespaced(&ability.path[0], ability_name)
                .cloned()
        {
            return self.lookup_dynamic_method(&dynamic, method_name, arg_tys, span);
        }

        // Module-declared abilities are used by bare name and take
        // precedence over bare-name builtins (Exception), mirroring how
        // local enums shadow the prelude. Namespaced prelude abilities
        // are unaffected: user declarations never register under a path.
        if ability.path.is_empty()
            && let Some(dynamic) = self.ability_resolver.get_dynamic(ability_name).cloned()
        {
            return self.lookup_dynamic_method(&dynamic, method_name, arg_tys, span);
        }

        // A namespaced dynamic that wasn't matched above was named bare or
        // under the wrong qualifier: performing it requires its namespace.
        if let Some(namespace) = self.ability_resolver.dynamic_namespace_of(ability_name) {
            return Err(type_error(
                TypeErrorKind::AbilityRequiresNamespace {
                    ability: ability_name.clone(),
                    expected_namespace: Arc::clone(namespace),
                },
                span,
            ));
        }

        let ability_id = self.ability_name_to_id(ability_name).ok_or_else(|| {
            type_error(
                TypeErrorKind::UnknownAbility {
                    name: ability_name.clone(),
                },
                span,
            )
        })?;

        // Builtin descriptors declare full signatures too (their type
        // variables arrive as `Hole` and resolve to fresh inference
        // variables); arguments are unified against the declared parameter
        // types exactly like dynamic abilities — a perform like
        // `Exception::throw!(42)` must not type-check.
        let Some((params, result_ty)) = self.ability_method_signature(ability_id, method_name)
        else {
            return Err(type_error(
                TypeErrorKind::UnknownAbilityMethod {
                    ability: ability_name.clone(),
                    method: method_name.into(),
                },
                span,
            ));
        };

        if params.len() != arg_tys.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: params.len(),
                    actual: arg_tys.len(),
                },
                span,
            ));
        }
        for (param, arg) in params.iter().zip(arg_tys) {
            self.unify(param, arg, span)?;
        }
        let result_ty = self.apply(&result_ty);

        Ok((ability_id, result_ty, AbilitySet::Empty))
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
            let param = param.substitute(&subst);
            self.unify(&param, arg, span)?;
        }

        let ret = method.ret.substitute(&subst);
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

    /// A distinct, recognizable AbilityId for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    fn span() -> (u32, u32) {
        (0, 0)
    }

    /// Create a qualified name with the `platform.` prefix.
    fn platform_ability(name: &str) -> QualifiedName {
        QualifiedName::qualified(vec!["platform"], name)
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
                params: vec![Type::String],
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
        infer
            .ability_resolver
            .register_dynamic_in_namespace("platform", printer_ability(7));

        let qualified = QualifiedName::qualified(vec!["platform"], "Printer");
        let (id, ret, _) = infer
            .lookup_ability_method(&qualified, "go", &[Type::String], span())
            .expect("qualified perform should resolve");
        assert_eq!(id, aid(7));
        assert_eq!(ret, Type::Unit);

        // Declared signatures are enforced: wrong argument type fails.
        let err = infer.lookup_ability_method(&qualified, "go", &[Type::Number], span());
        assert!(err.is_err(), "argument type mismatch should be rejected");

        // The wrong namespace does not resolve.
        let wrong = QualifiedName::qualified(vec!["other"], "Printer");
        assert!(
            infer
                .lookup_ability_method(&wrong, "go", &[Type::String], span())
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
    fn test_ability_name_to_id() {
        let mut infer = Infer::new();
        infer
            .ability_resolver
            .register_dynamic_in_namespace("platform", printer_ability(7));

        // Exception is the only engine builtin; prelude abilities resolve
        // by bare name too (effect rows, handler arms).
        assert_eq!(
            infer.ability_name_to_id("Exception"),
            Some(ambient_core::exception::ability_id())
        );
        assert_eq!(infer.ability_name_to_id("Printer"), Some(aid(7)));
        assert_eq!(infer.ability_name_to_id("Console"), None);
        assert_eq!(infer.ability_name_to_id("Unknown"), None);
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
    fn test_infer_ability_from_methods_uniqueness() {
        let mut infer = Infer::new();
        infer
            .ability_resolver
            .register_dynamic_in_namespace("platform", printer_ability(7));

        // "go" exists only in Printer.
        let methods: Vec<Arc<str>> = vec!["go".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(aid(7)));

        // "throw" exists only in Exception.
        let methods: Vec<Arc<str>> = vec!["throw".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(ambient_core::exception::ability_id()));
    }

    #[test]
    fn test_platform_namespace_required() {
        let mut infer = Infer::new();
        infer
            .ability_resolver
            .register_dynamic_in_namespace("platform", printer_ability(7));

        // A namespaced prelude ability performed bare should fail.
        let bare = QualifiedName::simple("Printer");
        let result = infer.lookup_ability_method(&bare, "go", &[Type::String], span());
        assert!(
            result.is_err(),
            "Printer without platform. prefix should fail"
        );

        // The same perform with the namespace succeeds.
        let qualified = platform_ability("Printer");
        let result = infer.lookup_ability_method(&qualified, "go", &[Type::String], span());
        assert!(result.is_ok(), "platform.Printer.go should succeed");
    }
}
