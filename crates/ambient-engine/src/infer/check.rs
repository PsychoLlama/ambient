//! Module-level type checking.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::types::{AbilityId, AbilitySet, Type, TypeVarId};

use super::env::{Scheme, TypeEnv};
use super::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};
use super::Infer;

/// Result of type checking a module.
#[derive(Debug)]
pub struct CheckResult {
    /// Type errors found during checking.
    pub errors: Vec<BoxedTypeError>,
    /// The typed module (with types filled in on expressions).
    pub module: crate::ast::Module,
}

impl CheckResult {
    /// Returns true if there were no errors.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the errors, consuming the result.
    #[must_use]
    pub fn into_errors(self) -> Vec<BoxedTypeError> {
        self.errors
    }
}

/// Check a module for type errors.
///
/// This function performs module-level type inference:
/// 1. Collects all function signatures into the type environment
/// 2. Type-checks each function body
/// 3. Verifies return types match declared types
/// 4. Returns all accumulated type errors
///
/// # Example
///
/// ```ignore
/// let module = ambient_parser::parse(source)?;
/// let result = check_module(module);
/// if !result.is_ok() {
///     for error in &result.errors {
///         eprintln!("Type error: {}", error);
///     }
/// }
/// ```
#[must_use]
pub fn check_module(mut module: crate::ast::Module) -> CheckResult {
    let mut infer = Infer::new();
    let mut errors = Vec::new();
    let mut env = TypeEnv::new();

    // Phase 1: Collect all function signatures into the environment.
    // This allows functions to call each other regardless of definition order.
    let mut function_schemes: Vec<(BindingId, Arc<str>, Scheme)> = Vec::new();
    let mut next_binding_id: BindingId = 1_000_000; // Start high to avoid collisions

    for item in &module.items {
        if let crate::ast::ItemKind::Function(func) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;

            // Build the function type from its signature
            let scheme = build_function_scheme(&mut infer, func);
            function_schemes.push((binding_id, Arc::clone(&func.name), scheme));
        }
    }

    // Add all function schemes to the environment
    for (id, name, scheme) in &function_schemes {
        env.insert(*id, Arc::clone(name), scheme.clone());
    }

    // Phase 2: Type-check each function body.
    for item in &mut module.items {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            // Reset ability tracking for each function
            infer.reset_abilities();

            // Create function-local environment with parameters
            let mut func_env = env.extend();

            // Build expected return type
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            // Add parameters to the environment
            let mut param_types = Vec::new();
            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
                param_types.push(param_ty.clone());
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            // Infer the body type
            match infer.infer_expr(&func_env, &mut func.body) {
                Ok(body_ty) => {
                    // Check return type matches if declared
                    if let Some(ref expected) = expected_ret_ty {
                        let span = (func.body.span.start, func.body.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, span) {
                            errors.push(e.with_context(format!(
                                "in function `{}`: return type mismatch",
                                func.name
                            )));
                        }
                    }

                    // Verify declared abilities match inferred abilities
                    let inferred_abilities = infer.current_abilities().clone();
                    if !func.abilities.is_empty() {
                        // Convert declared abilities to AbilitySet
                        let declared: Vec<AbilityId> = func
                            .abilities
                            .iter()
                            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
                            .collect();
                        let declared_set = AbilitySet::from_abilities(declared);

                        // Check that inferred abilities are a subset of declared
                        if let AbilitySet::Concrete(inferred_ids) = &inferred_abilities {
                            for ability_id in inferred_ids {
                                if !declared_set.contains(*ability_id) {
                                    let span = (item.span.start, item.span.end);
                                    errors.push(Box::new(
                                        TypeError::new(
                                            TypeErrorKind::MissingAbility {
                                                required: *ability_id,
                                                available: declared_set.clone(),
                                            },
                                            span,
                                        )
                                        .with_context(
                                            format!(
                                        "function `{}` uses ability #{} but doesn't declare it",
                                        func.name, ability_id
                                    ),
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in function `{}`", func.name)));
                }
            }
        }

        // Type-check constants
        if let crate::ast::ItemKind::Const(const_def) = &mut item.kind {
            infer.reset_abilities();
            let expected_ty = infer.resolve_holes(&const_def.ty);

            match infer.infer_expr(&env, &mut const_def.value) {
                Ok(actual_ty) => {
                    let span = (const_def.value.span.start, const_def.value.span.end);
                    if let Err(e) = infer.unify(&expected_ty, &actual_ty, span) {
                        errors.push(e.with_context(format!(
                            "in constant `{}`: type mismatch",
                            const_def.name
                        )));
                    }
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in constant `{}`", const_def.name)));
                }
            }
        }
    }

    CheckResult { errors, module }
}

/// Build a type scheme for a function from its signature.
fn build_function_scheme(infer: &mut Infer, func: &crate::ast::FunctionDef) -> Scheme {
    // Collect type variables from type parameters
    let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
    let mut quantified_vars = Vec::new();

    for (idx, tp) in func.type_params.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let var_id = idx as TypeVarId;
        type_var_map.insert(Arc::clone(&tp.name), var_id);
        quantified_vars.push(var_id);
    }

    // Build parameter types
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => substitute_type_params(ty, &type_var_map),
            None => infer.fresh(),
        })
        .collect();

    // Build return type
    let ret_ty = match &func.ret_ty {
        Some(ty) => substitute_type_params(ty, &type_var_map),
        None => infer.fresh(),
    };

    // Build ability set from declared abilities
    let abilities = if func.abilities.is_empty() {
        AbilitySet::Empty
    } else {
        let ability_ids: Vec<AbilityId> = func
            .abilities
            .iter()
            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
            .collect();
        AbilitySet::from_abilities(ability_ids)
    };

    let fn_ty = Type::function_with_abilities(param_types, ret_ty, abilities);

    if quantified_vars.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly(quantified_vars, fn_ty)
    }
}

/// Substitute type parameters in a type with type variables.
fn substitute_type_params(ty: &Type, type_var_map: &HashMap<Arc<str>, TypeVarId>) -> Type {
    match ty {
        Type::Named(named) => {
            // Check if this is a type parameter reference
            if named.args.is_empty() {
                if let Some(&var_id) = type_var_map.get(&named.name) {
                    return Type::var(var_id);
                }
            }
            // Otherwise, recursively substitute in type arguments
            Type::Named(crate::types::NamedType::new(
                Arc::clone(&named.name),
                named
                    .args
                    .iter()
                    .map(|arg| substitute_type_params(arg, type_var_map))
                    .collect(),
            ))
        }
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_type_params(p, type_var_map))
                .collect(),
            substitute_type_params(&f.ret, type_var_map),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_type_params(e, type_var_map))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_type_params(t, type_var_map)))
                .collect(),
        )),
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}
