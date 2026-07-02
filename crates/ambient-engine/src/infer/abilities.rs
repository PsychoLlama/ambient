//! Ability type inference and lookup.
//!
//! This module handles:
//! - Ability name/ID conversion
//! - Method signature lookup
//! - Ability tracking during inference

use std::sync::Arc;

use super::{type_error, Infer, InferResult, TypeErrorKind};
use crate::ability_resolver::EngineTypeFactory;
use crate::ast::QualifiedName;
use crate::types::{AbilityId, AbilitySet, Type};

/// Abilities that live under the `runtime` namespace.
const RUNTIME_ABILITIES: &[&str] = &["Console", "Time", "Random", "Log", "Network", "Execute"];

/// Check if an ability requires the `runtime.` namespace prefix.
fn is_runtime_ability(name: &str) -> bool {
    RUNTIME_ABILITIES.contains(&name)
}

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

    /// Get the method signatures for an ability using the resolver.
    ///
    /// Returns a list of (`method_name`, `param_count`, `return_type`) tuples.
    pub(crate) fn get_ability_method_signatures(
        &self,
        ability_id: AbilityId,
    ) -> Vec<(String, usize, Type)> {
        let factory = EngineTypeFactory;
        self.ability_resolver
            .get_method_signatures(ability_id, &factory)
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

        // Validate namespace for runtime abilities
        if is_runtime_ability(ability_name) {
            let has_runtime_prefix =
                ability.path.len() == 1 && ability.path[0].as_ref() == "runtime";
            if !has_runtime_prefix {
                return Err(type_error(
                    TypeErrorKind::AbilityRequiresNamespace {
                        ability: ability_name.clone(),
                        expected_namespace: "runtime",
                    },
                    span,
                ));
            }
        }

        // Module-declared abilities are used by bare name and take
        // precedence over bare-name builtins (Exception), mirroring how
        // local enums shadow the prelude. `runtime.`-namespaced abilities
        // are unaffected: user declarations never register under a path.
        if ability.path.is_empty() && !is_runtime_ability(ability_name) {
            if let Some(dynamic) = self.ability_resolver.get_dynamic(ability_name).cloned() {
                return self.lookup_dynamic_method(&dynamic, method_name, arg_tys, span);
            }
        }

        let ability_id = self.ability_name_to_id(ability_name).ok_or_else(|| {
            type_error(
                TypeErrorKind::UnknownAbility {
                    name: ability_name.clone(),
                },
                span,
            )
        })?;

        // Look up the return type from the resolver.
        // Builtin descriptors produce `Hole` for their type variables
        // (e.g. Execute.run's R); resolve to fresh inference variables so
        // the result unifies with whatever the call site expects.
        let factory = EngineTypeFactory;
        let result_ty = self
            .ability_resolver
            .get_method_return_type(ability_name, method_name, &factory)
            .ok_or_else(|| {
                type_error(
                    TypeErrorKind::UnknownAbilityMethod {
                        ability: ability_name.clone(),
                        method: method_name.into(),
                    },
                    span,
                )
            })?;
        let result_ty = self.resolve_holes(&result_ty);

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

    /// Create a qualified name with the `runtime.` prefix.
    fn runtime_ability(name: &str) -> QualifiedName {
        QualifiedName::qualified(vec!["runtime"], name)
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
        let infer = Infer::new();
        assert_eq!(
            infer.ability_name_to_id("Console"),
            Some(ambient_runtime::console::ability_id())
        );
        assert_eq!(
            infer.ability_name_to_id("Exception"),
            Some(ambient_core::exception::ability_id())
        );
        assert_eq!(
            infer.ability_name_to_id("Time"),
            Some(ambient_runtime::time::ability_id())
        );
        assert_eq!(
            infer.ability_name_to_id("Random"),
            Some(ambient_runtime::random::ability_id())
        );
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
        let infer = Infer::new();

        // "print" exists in Console
        let methods: Vec<Arc<str>> = vec!["print".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(ambient_runtime::console::ability_id()));

        // "throw" exists only in Exception
        let methods: Vec<Arc<str>> = vec!["throw".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(ambient_core::exception::ability_id()));

        // "now" exists only in Time
        let methods: Vec<Arc<str>> = vec!["now".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(ambient_runtime::time::ability_id()));
    }

    #[test]
    fn test_runtime_namespace_required() {
        let mut infer = Infer::new();

        // Console without runtime prefix should fail
        let console_no_prefix = QualifiedName::simple("Console");
        let result =
            infer.lookup_ability_method(&console_no_prefix, "print", &[Type::String], span());
        assert!(
            result.is_err(),
            "Console without runtime. prefix should fail"
        );

        // Console with runtime prefix should succeed
        let console_with_prefix = runtime_ability("Console");
        let result =
            infer.lookup_ability_method(&console_with_prefix, "print", &[Type::String], span());
        assert!(result.is_ok(), "runtime.Console.print should succeed");
    }
}
