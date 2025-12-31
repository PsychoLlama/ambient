//! Module-level type checking.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::AbilityResolver;
use crate::ast::BindingId;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};
use crate::types::{AbilityId, AbilitySet, TraitDef, TraitId, TraitMethodDef, Type, TypeVarId};
use uuid::Uuid;

use super::env::{Scheme, TypeEnv};
use super::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};
use super::expr::substitute_self;
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

    // Phase 1a: Register all type aliases so parameter types resolve correctly.
    register_type_aliases(&mut infer, &module);

    // Phase 1b: Register all trait definitions.
    register_traits(&mut infer, &module);

    // Phase 1c: Collect all function signatures into the environment.
    collect_function_signatures(&mut infer, &module, &mut env);

    // Phase 2: Type-check impl blocks.
    check_impls(&mut infer, &mut module, &env, &mut errors);

    // Phase 3: Type-check each function body.
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

/// Register all type aliases from a module into the inferencer.
fn register_type_aliases(infer: &mut Infer, module: &crate::ast::Module) {
    for item in &module.items {
        if let crate::ast::ItemKind::TypeAlias(type_alias) = &item.kind {
            infer.register_type_alias(Arc::clone(&type_alias.name), type_alias.ty.clone());
        }
    }
}

/// Register all trait definitions from a module into the trait registry.
fn register_traits(infer: &mut Infer, module: &crate::ast::Module) {
    for item in &module.items {
        if let crate::ast::ItemKind::Trait(trait_def) = &item.kind {
            let trait_id = infer.trait_registry.fresh_id();

            // Build method definitions
            let methods: Vec<TraitMethodDef> = trait_def
                .methods
                .iter()
                .map(|m| {
                    TraitMethodDef::new(
                        Arc::clone(&m.name),
                        m.has_self,
                        m.params.iter().map(|(_, ty)| ty.clone()).collect(),
                        m.ret_ty.clone(),
                    )
                })
                .collect();

            // Create and register the trait definition
            let def = TraitDef {
                id: trait_id,
                name: Arc::clone(&trait_def.name),
                type_params: Vec::new(), // TODO: Handle type params properly
                methods,
                supertraits: Vec::new(), // TODO: Resolve supertrait references
            };

            infer.trait_registry.register_trait(def);
        }
    }
}

/// Check impl blocks and register implementations.
fn check_impls(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
) {
    for item in &mut module.items {
        if let crate::ast::ItemKind::Impl(impl_def) = &mut item.kind {
            check_single_impl(infer, impl_def, item.span, env, errors);
        }
    }
}

/// Check a single impl block.
fn check_single_impl(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    item_span: crate::ast::Span,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
) {
    let span = (item_span.start, item_span.end);

    // Look up the trait
    let Some(trait_id) = infer.trait_registry.lookup_trait(&impl_def.trait_name.name) else {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::UnknownTrait {
                name: Arc::clone(&impl_def.trait_name.name),
            },
            span,
        )));
        return;
    };

    // Verify the implementing type is nominal
    let for_type = infer.resolve_holes(&impl_def.for_type);
    let Type::Nominal(nominal_type) = &for_type else {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::TraitOnStructuralType {
                trait_name: Arc::clone(&impl_def.trait_name.name),
                ty: for_type.clone(),
            },
            span,
        )));
        return;
    };

    // Get the trait definition to check method signatures
    let Some(trait_def) = infer.trait_registry.get_trait(trait_id).cloned() else {
        return;
    };

    // Check each method in the impl
    check_impl_methods(infer, impl_def, &trait_def, &for_type, env, errors);

    // Check that all required methods are implemented
    check_impl_completeness(impl_def, &trait_def, span, errors);

    // Register the impl with placeholder hashes for each method
    // These hashes uniquely identify each impl method for later resolution during compilation
    let mut impl_record = crate::types::TraitImpl::new(trait_id, nominal_type.clone());
    for method in &mut impl_def.methods {
        // Generate a placeholder hash based on trait_id, type UUID, and method name
        // This hash is used during type checking to resolve method calls
        let hash = generate_impl_method_hash(trait_id, &nominal_type.uuid, &method.name);
        impl_record.methods.insert(Arc::clone(&method.name), hash);
        // Also store the hash in the method AST for use during compilation
        method.resolved_hash = Some(hash);
    }
    infer.trait_registry.register_impl(impl_record);
}

/// Check all methods in an impl block.
fn check_impl_methods(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    trait_def: &TraitDef,
    for_type: &Type,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
) {
    for method in &mut impl_def.methods {
        let trait_method = trait_def
            .methods
            .iter()
            .find(|m| m.name.as_ref() == method.name.as_ref());

        let Some(tm) = trait_method else {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::MethodNotFound {
                    method: Arc::clone(&method.name),
                    ty: Type::Named(crate::types::NamedType::simple(Arc::clone(
                        &impl_def.trait_name.name,
                    ))),
                },
                (method.span.start, method.span.end),
            )));
            continue;
        };

        // Type-check the method body
        infer.reset_abilities();
        let mut func_env = env.extend();

        // Add self parameter
        if tm.has_self {
            func_env.insert_mono(method.self_id, Arc::from("self"), for_type.clone());
        }

        // Add other parameters, substituting Self with the implementing type
        for (param, expected_ty) in method.params.iter().zip(tm.params.iter()) {
            // Substitute Self with for_type in the expected type from trait method
            let expected_ty_substituted = substitute_self(expected_ty, for_type);
            let param_ty = param.ty.as_ref().map_or_else(
                || expected_ty_substituted.clone(),
                |ty| infer.resolve_holes(ty),
            );
            func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
        }

        // Infer body type and check against expected return type
        // Substitute Self with for_type in the expected return type
        let expected_ret = substitute_self(&tm.ret, for_type);
        match infer.infer_expr(&func_env, &mut method.body) {
            Ok(body_ty) => {
                let method_span = (method.span.start, method.span.end);
                if let Err(e) = infer.unify(&expected_ret, &body_ty, method_span) {
                    errors.push(e.with_context(format!("in impl method `{}`", method.name)));
                }
            }
            Err(e) => {
                errors.push(e.with_context(format!("in impl method `{}`", method.name)));
            }
        }
    }
}

/// Check that all required trait methods are implemented.
fn check_impl_completeness(
    impl_def: &crate::ast::ImplDef,
    trait_def: &TraitDef,
    span: (u32, u32),
    errors: &mut Vec<BoxedTypeError>,
) {
    for trait_method in &trait_def.methods {
        let implemented = impl_def
            .methods
            .iter()
            .any(|m| m.name.as_ref() == trait_method.name.as_ref());
        if !implemented {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::ImplMissingMethod {
                    trait_name: Arc::clone(&impl_def.trait_name.name),
                    method: Arc::clone(&trait_method.name),
                },
                span,
            )));
        }
    }
}

/// Collect function signatures into the environment.
fn collect_function_signatures(infer: &mut Infer, module: &crate::ast::Module, env: &mut TypeEnv) {
    let mut next_binding_id: BindingId = 1_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Function(func) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;
            let scheme = build_function_scheme(infer, func);
            env.insert(binding_id, Arc::clone(&func.name), scheme);
        }
    }
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

    // Build parameter types, resolving type aliases
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => {
                let substituted = substitute_type_params(ty, &type_var_map);
                infer.resolve_holes(&substituted)
            }
            None => infer.fresh(),
        })
        .collect();

    // Build return type, resolving type aliases
    let ret_ty = match &func.ret_ty {
        Some(ty) => {
            let substituted = substitute_type_params(ty, &type_var_map);
            infer.resolve_holes(&substituted)
        }
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

/// Check a module with cross-module imports resolved.
///
/// This is the main entry point for cross-module type checking. It resolves
/// imports from the registry and includes their types in the initial environment.
///
/// # Arguments
///
/// * `module` - The module to type check
/// * `module_path` - The path of this module in the package
/// * `registry` - The module registry containing all loaded modules
#[must_use]
pub fn check_module_with_registry(
    mut module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> CheckResult {
    let mut infer = Infer::new();
    let mut errors = Vec::new();

    // Build initial environment from imports
    let mut env = build_import_env(&mut infer, module_path, registry, &mut errors);

    // Phase 1a: Register all type aliases so parameter types resolve correctly.
    register_type_aliases(&mut infer, &module);

    // Phase 1b: Register all trait definitions.
    register_traits(&mut infer, &module);

    // Phase 1c: Collect all function signatures into the environment.
    collect_function_signatures(&mut infer, &module, &mut env);

    // Phase 2: Type-check impl blocks.
    check_impls(&mut infer, &mut module, &env, &mut errors);

    // Phase 3: Type-check each function body
    for item in &mut module.items {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            infer.reset_abilities();

            let mut func_env = env.extend();
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            let mut param_types = Vec::new();
            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
                param_types.push(param_ty.clone());
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            match infer.infer_expr(&func_env, &mut func.body) {
                Ok(body_ty) => {
                    if let Some(ref expected) = expected_ret_ty {
                        let span = (func.body.span.start, func.body.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, span) {
                            errors.push(e.with_context(format!(
                                "in function `{}`: return type mismatch",
                                func.name
                            )));
                        }
                    }

                    let inferred_abilities = infer.current_abilities().clone();
                    if !func.abilities.is_empty() {
                        let declared: Vec<AbilityId> = func
                            .abilities
                            .iter()
                            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
                            .collect();
                        let declared_set = AbilitySet::from_abilities(declared);

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

/// Check a module with cross-module support and a custom ability resolver.
///
/// This variant allows specifying which abilities are available at compile time,
/// which is useful for LSP and other tools that need to respect package configuration.
///
/// # Arguments
///
/// * `module` - The module to type check
/// * `module_path` - The path of this module in the package
/// * `registry` - The module registry containing all loaded modules
/// * `resolver` - The ability resolver specifying available abilities
#[must_use]
pub fn check_module_with_registry_and_resolver(
    mut module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    resolver: AbilityResolver,
) -> CheckResult {
    let mut infer = Infer::with_resolver(resolver);
    let mut errors = Vec::new();

    // Build initial environment from imports
    let mut env = build_import_env(&mut infer, module_path, registry, &mut errors);

    // Phase 1a: Register all type aliases so parameter types resolve correctly.
    register_type_aliases(&mut infer, &module);

    // Phase 1b: Register all trait definitions.
    register_traits(&mut infer, &module);

    // Phase 1c: Collect all function signatures into the environment.
    collect_function_signatures(&mut infer, &module, &mut env);

    // Phase 2: Type-check impl blocks.
    check_impls(&mut infer, &mut module, &env, &mut errors);

    // Phase 3: Type-check each function body
    for item in &mut module.items {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            infer.reset_abilities();

            let mut func_env = env.extend();
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            let mut param_types = Vec::new();
            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
                param_types.push(param_ty.clone());
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            match infer.infer_expr(&func_env, &mut func.body) {
                Ok(body_ty) => {
                    if let Some(ref expected) = expected_ret_ty {
                        let span = (func.body.span.start, func.body.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, span) {
                            errors.push(e.with_context(format!(
                                "in function `{}`: return type mismatch",
                                func.name
                            )));
                        }
                    }

                    let inferred_abilities = infer.current_abilities().clone();
                    if !func.abilities.is_empty() {
                        let declared: Vec<AbilityId> = func
                            .abilities
                            .iter()
                            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
                            .collect();
                        let declared_set = AbilitySet::from_abilities(declared);

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
                                        .with_context(format!("in function `{}`", func.name)),
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

/// Build a type environment from imported modules.
///
/// This processes the imports in the module and adds type schemes for
/// each imported symbol to the environment.
fn build_import_env(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) -> TypeEnv {
    let mut env = TypeEnv::new();
    let mut next_binding_id: BindingId = 2_000_000; // Start high to avoid collisions

    // Get resolved imports for this module
    let imports = match registry.resolve_imports(module_path) {
        Ok(imports) => imports,
        Err(e) => {
            // Module not in registry - return empty env
            // This can happen for the root module being checked
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::CannotInfer {
                    hint: format!("import resolution failed: {e}"),
                },
                (0, 0),
            )));
            return env;
        }
    };

    for (name, resolved_import) in imports {
        match resolved_import {
            ResolvedImport::Module(target_path) => {
                // For module imports, we create a synthetic module type
                // For now, we skip this as it requires qualified name resolution
                // TODO: Support `utils.helper()` syntax
                let _ = target_path;
            }
            ResolvedImport::Symbol {
                from_module,
                export_kind,
            } => {
                // Look up the symbol's type from the source module
                if let Some(module_info) = registry.get(&from_module) {
                    if let Some(scheme) =
                        get_symbol_scheme(infer, &module_info.module, &name, export_kind)
                    {
                        let binding_id = next_binding_id;
                        next_binding_id += 1;
                        env.insert(binding_id, name, scheme);
                    }
                }
            }
        }
    }

    env
}

/// Get the type scheme for a symbol from a module's AST.
fn get_symbol_scheme(
    infer: &mut Infer,
    module: &crate::ast::Module,
    name: &str,
    kind: ExportKind,
) -> Option<Scheme> {
    for item in &module.items {
        match (&item.kind, kind) {
            (crate::ast::ItemKind::Function(func), ExportKind::Function) => {
                if func.name.as_ref() == name {
                    return Some(build_function_scheme(infer, func));
                }
            }
            (crate::ast::ItemKind::Const(const_def), ExportKind::Const) => {
                if const_def.name.as_ref() == name {
                    return Some(Scheme::mono(const_def.ty.clone()));
                }
            }
            (crate::ast::ItemKind::Enum(enum_def), ExportKind::Enum) => {
                if enum_def.name.as_ref() == name {
                    // For enum types, we return the type itself
                    // This is simplified - a full implementation would handle generic enums
                    let ty = Type::Named(crate::types::NamedType::new(
                        Arc::clone(&enum_def.name),
                        vec![],
                    ));
                    return Some(Scheme::mono(ty));
                }
            }
            (crate::ast::ItemKind::Enum(enum_def), ExportKind::EnumVariant) => {
                // Look for the variant in the enum
                for variant in &enum_def.variants {
                    if variant.name.as_ref() == name {
                        // Return the variant constructor type
                        // For Some(T) -> (T) -> Option<T>
                        // For None -> Option<T>
                        let enum_ty = Type::Named(crate::types::NamedType::new(
                            Arc::clone(&enum_def.name),
                            vec![], // TODO: handle generic enum parameters
                        ));

                        let scheme = if let Some(ref payload) = variant.payload {
                            // Constructor function: (payload) -> Enum
                            Scheme::mono(Type::function(vec![payload.clone()], enum_ty))
                        } else {
                            // Constant: Enum
                            Scheme::mono(enum_ty)
                        };
                        return Some(scheme);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Generate a deterministic hash for an impl method.
/// This hash uniquely identifies the method based on:
/// - The trait ID
/// - The implementing type's UUID
/// - The method name
///
/// This is used during type checking to resolve method calls.
/// During compilation, this same hash is used to identify which
/// impl method code to emit.
fn generate_impl_method_hash(
    trait_id: TraitId,
    type_uuid: &Uuid,
    method_name: &str,
) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"impl_method:");
    hasher.update(&trait_id.to_le_bytes());
    hasher.update(b":");
    hasher.update(type_uuid.as_bytes());
    hasher.update(b":");
    hasher.update(method_name.as_bytes());
    hasher.finalize()
}
