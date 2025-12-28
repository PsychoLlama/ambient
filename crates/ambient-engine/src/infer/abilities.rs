//! Ability type inference and lookup.
//!
//! This module handles:
//! - Ability name/ID conversion
//! - Method signature lookup
//! - Async.all/race polymorphic type inference
//! - Ability tracking during inference

use std::sync::Arc;

use super::{type_error, Infer, InferResult, TypeErrorKind};
use crate::ability_resolver::EngineTypeFactory;
use crate::ast::QualifiedName;
use crate::types::{AbilityId, AbilitySet, Type};

/// Abilities that live under the `runtime` namespace.
const RUNTIME_ABILITIES: &[&str] = &[
    "Console", "Time", "Random", "Async", "Log", "Network", "Execute",
];

/// Check if an ability requires the `runtime.` namespace prefix.
fn is_runtime_ability(name: &str) -> bool {
    RUNTIME_ABILITIES.contains(&name)
}

impl Infer {
    // ─────────────────────────────────────────────────────────────────────────
    // Ability lookup helpers (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────

    /// Well-known ability ID for Async (needed for special polymorphic handling).
    pub(crate) const ABILITY_ASYNC: AbilityId = 0x0005;

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
    /// For most abilities, the additional abilities set is empty. For `Async.all` and `Async.race`,
    /// it includes the underlying ability from the suspended ability values being performed.
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

        let ability_id = self.ability_name_to_id(ability_name).ok_or_else(|| {
            type_error(
                TypeErrorKind::UnknownAbility {
                    name: ability_name.clone(),
                },
                span,
            )
        })?;

        // Special handling for Async methods which are polymorphic
        if ability_id == Self::ABILITY_ASYNC {
            let (result_ty, additional_abilities) = match method_name {
                "all" => {
                    // Async.all: List<Ability<T, A!>> -> List<T> with Async, A
                    self.infer_async_all_type(arg_tys, span)?
                }
                "race" => {
                    // Async.race: List<Ability<T, A!>> -> T with Async, A
                    self.infer_async_race_type(arg_tys, span)?
                }
                _ => {
                    return Err(type_error(
                        TypeErrorKind::UnknownAbilityMethod {
                            ability: ability_name.clone(),
                            method: method_name.into(),
                        },
                        span,
                    ))
                }
            };
            return Ok((ability_id, result_ty, additional_abilities));
        }

        // For other abilities, look up the return type from the resolver
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

        Ok((ability_id, result_ty, AbilitySet::Empty))
    }

    /// Infer the result type for `Async.all(ops)` where `ops: List<Ability<T, A!>>`.
    /// Returns `(List<T>, A)` - the result type and the underlying ability to require.
    fn infer_async_all_type(
        &mut self,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        // Async.all takes exactly one argument
        if arg_tys.len() != 1 {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: 1,
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        // Extract T and A from List<Ability<T, A!>>
        let (element_result_ty, underlying_ability) =
            self.extract_list_ability_types(&arg_tys[0], span)?;

        // Return List<T>
        let result_ty = Type::named("List", vec![element_result_ty]);
        Ok((result_ty, underlying_ability))
    }

    /// Infer the result type for `Async.race(ops)` where `ops: List<Ability<T, A!>>`.
    /// Returns `(T, A)` - the result type and the underlying ability to require.
    fn infer_async_race_type(
        &mut self,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        // Async.race takes exactly one argument
        if arg_tys.len() != 1 {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: 1,
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        // Extract T and A from List<Ability<T, A!>>
        let (element_result_ty, underlying_ability) =
            self.extract_list_ability_types(&arg_tys[0], span)?;

        // Return T (just the element type, not wrapped in List)
        Ok((element_result_ty, underlying_ability))
    }

    /// Extract T and A from a type that should be `List<Ability<T, A!>>`.
    ///
    /// Returns the result type T and the ability set A.
    fn extract_list_ability_types(
        &mut self,
        ty: &Type,
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        let ty = self.apply(ty);

        // Create fresh type variables for T and A
        let expected_t = self.fresh();
        let expected_a = self.fresh_ability_var();
        let expected_ability_value = Type::ability_value(expected_t.clone(), expected_a.clone());
        let expected_list = Type::named("List", vec![expected_ability_value]);

        // Unify with the actual argument type
        self.unify(&ty, &expected_list, span)?;

        // Apply substitutions to get the concrete types
        let result_ty = self.apply(&expected_t);
        let ability_set = self.apply_abilities(&expected_a);

        Ok((result_ty, ability_set))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::QualifiedName;
    use crate::infer::Infer;
    use crate::types::{AbilityInfo, AbilityRegistry, AbilitySet, Type};

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
        infer.require_ability(1);
        assert!(infer.current_abilities().contains(1));

        // Require another ability
        infer.require_ability(2);
        assert!(infer.current_abilities().contains(1));
        assert!(infer.current_abilities().contains(2));

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
        assert_eq!(infer.ability_name_to_id("Console"), Some(1));
        assert_eq!(infer.ability_name_to_id("Exception"), Some(2));
        assert_eq!(infer.ability_name_to_id("Time"), Some(3));
        assert_eq!(infer.ability_name_to_id("Random"), Some(4));
        assert_eq!(infer.ability_name_to_id("Async"), Some(5));
        assert_eq!(infer.ability_name_to_id("Unknown"), None);
    }

    #[test]
    fn test_require_ability_with_registry() {
        let mut registry = AbilityRegistry::new();

        // IO is ability 1
        registry.register(1, AbilityInfo::new("IO"));

        // FileSystem (2) depends on IO (1)
        registry.register(2, AbilityInfo::new("FileSystem").with_dependency(1));

        let mut infer = Infer::with_registry(registry);

        // When we require FileSystem, IO should also be required
        infer.require_ability(2);

        let abilities = infer.current_abilities();
        if let AbilitySet::Concrete(ids) = abilities {
            assert!(ids.contains(&1), "IO should be required");
            assert!(ids.contains(&2), "FileSystem should be required");
        } else {
            panic!("Expected concrete ability set");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Async type checking tests (Milestone 9)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_async_all_type_inference() {
        let mut infer = Infer::new();

        // Create argument type: List<Ability<string, Console!>>
        let ability_value = Type::ability_value(Type::String, AbilitySet::single(1)); // Console = 1
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.all with this argument
        let result = infer.lookup_ability_method(
            &runtime_ability("Async"),
            "all",
            &[list_of_abilities],
            span(),
        );
        assert!(
            result.is_ok(),
            "Async.all should accept List<Ability<T, A!>>"
        );

        let (ability_id, result_ty, additional_abilities) = result.unwrap();

        // Should return Async ability ID
        assert_eq!(ability_id, 5, "Should return Async ability ID");

        // Should return List<string> (the result type wrapped in List)
        if let Type::Named(named) = &result_ty {
            assert_eq!(named.name.as_ref(), "List");
            assert_eq!(named.args.len(), 1);
            assert_eq!(named.args[0], Type::String);
        } else {
            panic!("Expected Named type List<string>, got {:?}", result_ty);
        }

        // Should include Console in additional abilities
        assert!(
            matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&1)),
            "Should include Console ability in additional_abilities"
        );
    }

    #[test]
    fn test_async_race_type_inference() {
        let mut infer = Infer::new();

        // Create argument type: List<Ability<number, Time!>>
        let ability_value = Type::ability_value(Type::Number, AbilitySet::single(3)); // Time = 3
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.race with this argument
        let result = infer.lookup_ability_method(
            &runtime_ability("Async"),
            "race",
            &[list_of_abilities],
            span(),
        );
        assert!(
            result.is_ok(),
            "Async.race should accept List<Ability<T, A!>>"
        );

        let (ability_id, result_ty, additional_abilities) = result.unwrap();

        // Should return Async ability ID
        assert_eq!(ability_id, 5, "Should return Async ability ID");

        // Should return number (the unwrapped result type)
        assert_eq!(
            result_ty,
            Type::Number,
            "Async.race should return T, not List<T>"
        );

        // Should include Time in additional abilities
        assert!(
            matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&3)),
            "Should include Time ability in additional_abilities"
        );
    }

    #[test]
    fn test_async_all_with_type_variable() {
        let mut infer = Infer::new();

        // Create a type variable for the result type
        let result_var = infer.fresh();
        let ability_var = infer.fresh_ability_var();

        // Create argument type: List<Ability<T, A!>> with fresh variables
        let ability_value = Type::ability_value(result_var.clone(), ability_var.clone());
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.all - should succeed with polymorphic types
        let result = infer.lookup_ability_method(
            &runtime_ability("Async"),
            "all",
            &[list_of_abilities],
            span(),
        );
        assert!(result.is_ok(), "Async.all should work with type variables");

        let (_, result_ty, _) = result.unwrap();

        // Result should be List<T> where T is the same variable
        if let Type::Named(named) = &result_ty {
            assert_eq!(named.name.as_ref(), "List");
            // The inner type should be related to our original type variable
            // (either the same or unified)
        } else {
            panic!("Expected Named type, got {:?}", result_ty);
        }
    }

    #[test]
    fn test_async_all_wrong_arity() {
        let mut infer = Infer::new();

        // Try calling Async.all with no arguments
        let result = infer.lookup_ability_method(&runtime_ability("Async"), "all", &[], span());
        assert!(
            result.is_err(),
            "Async.all should require exactly one argument"
        );

        // Try calling Async.all with two arguments
        let arg1 = Type::named(
            "List",
            vec![Type::ability_value(Type::String, AbilitySet::single(1))],
        );
        let arg2 = Type::named(
            "List",
            vec![Type::ability_value(Type::Number, AbilitySet::single(1))],
        );
        let result =
            infer.lookup_ability_method(&runtime_ability("Async"), "all", &[arg1, arg2], span());
        assert!(result.is_err(), "Async.all should not accept two arguments");
    }

    #[test]
    fn test_async_race_wrong_arity() {
        let mut infer = Infer::new();

        // Try calling Async.race with no arguments
        let result = infer.lookup_ability_method(&runtime_ability("Async"), "race", &[], span());
        assert!(
            result.is_err(),
            "Async.race should require exactly one argument"
        );
    }

    #[test]
    fn test_async_all_wrong_type() {
        let mut infer = Infer::new();

        // Try calling Async.all with a non-List type (e.g., just a number)
        let result =
            infer.lookup_ability_method(&runtime_ability("Async"), "all", &[Type::Number], span());
        assert!(
            result.is_err(),
            "Async.all should reject non-List arguments"
        );

        // Try calling Async.all with List<number> (not List<Ability<...>>)
        let list_of_numbers = Type::named("List", vec![Type::Number]);
        let result = infer.lookup_ability_method(
            &runtime_ability("Async"),
            "all",
            &[list_of_numbers],
            span(),
        );
        assert!(result.is_err(), "Async.all should reject List<number>");
    }

    #[test]
    fn test_infer_ability_from_methods_uniqueness() {
        let infer = Infer::new();

        // "print" exists in Console
        let methods: Vec<Arc<str>> = vec!["print".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0001)); // Console

        // "throw" exists only in Exception
        let methods: Vec<Arc<str>> = vec!["throw".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0002)); // Exception

        // "now" exists only in Time
        let methods: Vec<Arc<str>> = vec!["now".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0003)); // Time
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
