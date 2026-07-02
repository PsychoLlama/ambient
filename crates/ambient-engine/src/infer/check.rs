//! Module-level type checking.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::AbilityResolver;
use crate::ast::BindingId;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};
use crate::types::{AbilityId, AbilitySet, TraitDef, TraitMethodDef, Type, TypeVarId};

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
pub fn check_module(module: crate::ast::Module) -> CheckResult {
    check_module_core(Infer::new(), module, None)
}

/// The shared checking pipeline behind all `check_module*` entry points.
///
/// Phases:
/// 1. Registration — foreign package items (if cross-module), imports, local
///    type aliases, traits, and function signatures.
/// 2. Impl blocks — method bodies checked, dispatch symbols assigned.
/// 3. Function/const bodies — types inferred; each unannotated private
///    function's ability variable is bound to its body's inferred effects.
/// 4. Ability enforcement — deferred until all bodies are checked so calls
///    to functions defined later (whose ability variables bind in phase 3)
///    resolve before their callers are judged.
fn check_module_core(
    mut infer: Infer,
    mut module: crate::ast::Module,
    cross_module: Option<(&ModulePath, &ModuleRegistry)>,
) -> CheckResult {
    let mut errors = Vec::new();

    // Phase 1: registration.
    let mut env = if let Some((module_path, registry)) = cross_module {
        // Make the rest of the package's types, traits, and impls visible
        // (signatures only). Runs before local registration and import
        // resolution so imported signatures resolve foreign nominal types.
        register_package_items(&mut infer, module_path, registry, &mut errors);
        build_import_env(&mut infer, module_path, registry, &mut errors)
    } else {
        TypeEnv::new()
    };

    register_type_aliases(&mut infer, &module);
    register_traits(&mut infer, &module);
    register_enums(&mut infer, &module, &mut env);
    register_abilities(&mut infer, &mut module, &mut errors);
    collect_function_signatures(&mut infer, &module, &mut env);

    // Phase 2: impl blocks.
    check_impls(&mut infer, &mut module, &env, &mut errors);

    // Phase 3: function and const bodies.
    // Records (item index, raw inferred abilities) for deferred enforcement.
    let mut inferred_abilities: Vec<(usize, AbilitySet)> = Vec::new();

    for (idx, item) in module.items.iter_mut().enumerate() {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            infer.reset_abilities();

            let mut func_env = env.extend();
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
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

                    bind_inferred_abilities(&mut infer, &env, func);
                    inferred_abilities.push((idx, infer.current_abilities().clone()));
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

    // Phase 4: enforce declared abilities with final substitutions applied.
    for (idx, inferred) in inferred_abilities {
        if let crate::ast::ItemKind::Function(func) = &module.items[idx].kind {
            enforce_declared_abilities(
                &infer,
                func,
                module.items[idx].span,
                &inferred,
                &mut errors,
            );
        }
    }

    errors.extend(infer.take_pending_errors());
    CheckResult { errors, module }
}

/// Bind an unannotated private function's ability variable to its body's
/// inferred effects, making the real effect set visible at call sites.
///
/// Annotated and public functions are skipped: their scheme carries the
/// declared (possibly empty) ability set, which enforcement checks instead.
fn bind_inferred_abilities(infer: &mut Infer, env: &TypeEnv, func: &crate::ast::FunctionDef) {
    if !func.abilities.is_empty() || func.is_public {
        return;
    }
    let Some(scheme) = env.get_by_name(&func.name) else {
        return;
    };
    let Type::Function(f) = &scheme.ty else {
        return;
    };
    let AbilitySet::Var(var_id) = f.abilities else {
        return;
    };

    // Call sites checked before this body may have unified the scheme's
    // variable with their own fresh variables (var → var links). Binding at
    // `var_id` directly would sever those links, leaving the callers'
    // variables dangling — follow the chain and bind its representative.
    let mut root = var_id;
    let mut seen = vec![var_id];
    while let Some(AbilitySet::Var(next)) = infer.ability_subst.get(&root) {
        if seen.contains(next) {
            break;
        }
        root = *next;
        seen.push(root);
    }

    let inferred = infer.apply_abilities(infer.current_abilities());

    // Self-recursion makes the body require the function's own variable:
    // `var = concrete ∪ var` solves to `var = concrete`.
    let bound = match inferred {
        AbilitySet::Var(id) if seen.contains(&id) => AbilitySet::Empty,
        AbilitySet::Row { concrete, tail } if seen.contains(&tail) => {
            AbilitySet::from_abilities(concrete)
        }
        other => other,
    };
    infer.ability_subst.insert(root, bound);
}

/// Verify that a function's inferred abilities are a subset of its declared
/// abilities.
///
/// Applies to annotated functions and to public functions (where no `with`
/// clause means "pure"). Unannotated private functions are skipped — their
/// abilities are inferred, not declared. Abilities that remain polymorphic
/// after substitution are not enforced; they are constrained at the call
/// sites that instantiate them.
fn enforce_declared_abilities(
    infer: &Infer,
    func: &crate::ast::FunctionDef,
    item_span: crate::ast::Span,
    inferred: &AbilitySet,
    errors: &mut Vec<BoxedTypeError>,
) {
    if func.abilities.is_empty() && !func.is_public {
        // Abilities were inferred (bind_inferred_abilities), nothing declared
        // to enforce against.
        return;
    }

    let inferred = infer.apply_abilities(inferred);

    let declared: Vec<AbilityId> = func
        .abilities
        .iter()
        .filter_map(|qn| infer.ability_name_to_id(&qn.name))
        .collect();
    let declared_set = AbilitySet::from_abilities(declared);

    let inferred_ids = match &inferred {
        AbilitySet::Concrete(ids) => ids.as_slice(),
        AbilitySet::Row { concrete, .. } => concrete.as_slice(),
        AbilitySet::Empty | AbilitySet::Var(_) | AbilitySet::Unresolved(_) => &[],
    };

    for ability_id in inferred_ids {
        if !declared_set.contains(*ability_id) {
            let span = (item_span.start, item_span.end);
            let name = infer
                .ability_id_to_name(*ability_id)
                .unwrap_or("<unknown>")
                .to_string();
            errors.push(Box::new(
                TypeError::new(
                    TypeErrorKind::MissingAbility {
                        required: *ability_id,
                        available: declared_set.clone(),
                    },
                    span,
                )
                .with_context(format!(
                    "function `{}` uses ability `{name}` but doesn't declare it",
                    func.name
                )),
            ));
        }
    }
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
            register_trait_def(infer, trait_def);
        }
    }
}

/// Register a single trait definition into the trait registry.
fn register_trait_def(infer: &mut Infer, trait_def: &crate::ast::TraitDef) {
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

/// Register types, traits, and impls declared in the *other* modules of the
/// package so they are visible while checking this module.
///
/// Foreign items are registered by signature only — their bodies were (or
/// will be) checked in their own module's check pass. Impls register the
/// dispatch mapping `(trait, type uuid) → method symbol`; the symbols are
/// resolved to content hashes at link time like any function name.
///
/// This runs before the current module's own registrations, so local
/// declarations shadow foreign ones on name collisions.
fn register_package_items(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) {
    let foreign_modules: Vec<_> = registry
        .all_modules()
        .filter(|info| &info.path != current_module)
        .collect();

    // Types and traits first: impl registration needs both resolvable.
    for info in &foreign_modules {
        register_type_aliases(infer, &info.module);
        register_traits(infer, &info.module);
    }

    for info in &foreign_modules {
        for item in &info.module.items {
            if let crate::ast::ItemKind::Impl(impl_def) = &item.kind {
                register_foreign_impl(infer, impl_def, errors);
            }
        }
    }
}

/// Register the dispatch mapping for an impl defined in another module.
///
/// Skips silently on unresolvable traits or non-nominal types: the impl's
/// own module reports those errors during its check pass.
fn register_foreign_impl(
    infer: &mut Infer,
    impl_def: &crate::ast::ImplDef,
    errors: &mut Vec<BoxedTypeError>,
) {
    let Some(trait_id) = infer.trait_registry.lookup_trait(&impl_def.trait_name.name) else {
        return;
    };
    let for_type = infer.resolve_holes(&impl_def.for_type);
    let Type::Nominal(nominal_type) = &for_type else {
        return;
    };

    let mut impl_record = crate::types::TraitImpl::new(trait_id, nominal_type.clone());
    for method in &impl_def.methods {
        let symbol = crate::types::impl_method_symbol(
            &nominal_type.uuid,
            &impl_def.trait_name.name,
            &method.name,
        );
        impl_record.methods.insert(Arc::clone(&method.name), symbol);
    }
    if infer.trait_registry.register_impl(impl_record).is_some() {
        // Two other modules implement the same trait for the same type.
        // Their dispatch symbols collide, so this is unresolvable ambiguity.
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::DuplicateImpl {
                trait_name: Arc::clone(&impl_def.trait_name.name),
                ty: for_type.clone(),
            },
            (impl_def.span.start, impl_def.span.end),
        )));
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

    // Register the impl, assigning each method its canonical function symbol.
    // The compiler registers method bodies under these symbols so they are
    // content-addressed like ordinary functions; call sites resolve the
    // symbol through the same name→hash table as regular calls.
    let mut impl_record = crate::types::TraitImpl::new(trait_id, nominal_type.clone());
    for method in &mut impl_def.methods {
        let symbol = crate::types::impl_method_symbol(
            &nominal_type.uuid,
            &impl_def.trait_name.name,
            &method.name,
        );
        impl_record
            .methods
            .insert(Arc::clone(&method.name), Arc::clone(&symbol));
        method.resolved_symbol = Some(symbol);
    }
    if infer.trait_registry.register_impl(impl_record).is_some() {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::DuplicateImpl {
                trait_name: Arc::clone(&impl_def.trait_name.name),
                ty: for_type.clone(),
            },
            span,
        )));
    }
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
            let scheme = build_function_scheme(infer, func, true);
            env.insert(binding_id, Arc::clone(&func.name), scheme);
        }
    }
}

/// Build a type scheme for a function from its signature.
///
/// `infer_abilities` controls what an absent `with` clause means: for local
/// private functions (true) the scheme gets a fresh ability variable that
/// [`bind_inferred_abilities`] later binds to the body's inferred effects;
/// for public or foreign functions (false) it means "pure".
fn build_function_scheme(
    infer: &mut Infer,
    func: &crate::ast::FunctionDef,
    infer_abilities: bool,
) -> Scheme {
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
        if infer_abilities && !func.is_public {
            infer.fresh_ability_var()
        } else {
            AbilitySet::Empty
        }
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

/// Register the module's `ability` declarations.
///
/// Each declaration's method signatures are resolved (type parameters
/// become quantified type variables, aliases expand), rendered to
/// canonical form, and hashed into the ability's content-addressed
/// identity. The resulting [`DynAbility`] joins the resolver so
/// perform/suspend/handle and `with` clauses see it exactly like a
/// builtin; the computed identity is stored back into the AST for the
/// compiler.
///
/// Declared dependencies (`ability Log with Console`) resolve against
/// abilities already known to the resolver — builtins or dynamics
/// registered earlier in the item list — and are recorded in the ability
/// registry so requiring the ability transitively requires them.
fn register_abilities(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) -> Vec<Arc<crate::ability_resolver::DynAbility>> {
    use crate::ability_resolver::{CanonicalTypeRenderer, DynAbility, DynMethod};

    let mut resolved = Vec::new();
    for item in &mut module.items {
        let crate::ast::ItemKind::Ability(def) = &mut item.kind else {
            continue;
        };

        // Resolve dependencies first: they must already be known.
        let mut dependencies = Vec::new();
        for dep in &def.dependencies {
            match infer.ability_resolver.name_to_id(&dep.name) {
                Some(id) => dependencies.push(id),
                None => {
                    errors.push(super::type_error(
                        TypeErrorKind::UnknownAbility {
                            name: Arc::clone(&dep.name),
                        },
                        (def.name_span.start, def.name_span.end),
                    ));
                }
            }
        }

        let mut methods = Vec::new();
        let mut canonical = Vec::new();
        #[allow(clippy::cast_possible_truncation)]
        for (idx, method) in def.methods.iter().enumerate() {
            // Type parameters become quantified variables, substituted
            // into the declared types.
            let mut param_map = HashMap::new();
            let mut quantified = Vec::new();
            for tp in &method.type_params {
                let var_id = infer.gen.fresh_id();
                param_map.insert(Arc::clone(&tp.name), var_id);
                quantified.push(var_id);
            }

            let params: Vec<Type> = method
                .params
                .iter()
                .map(|(_, ty)| infer.resolve_holes(&substitute_type_params(ty, &param_map)))
                .collect();
            let ret = infer.resolve_holes(&substitute_type_params(&method.ret_ty, &param_map));

            // One renderer per signature: variable numbering is
            // signature-local, by first occurrence.
            let mut renderer = CanonicalTypeRenderer::new();
            let canon_params: Vec<String> = params.iter().map(|p| renderer.render(p)).collect();
            let canon_ret = renderer.render(&ret);
            canonical.push((Arc::clone(&method.name), canon_params, canon_ret));

            methods.push(DynMethod {
                id: idx as u16,
                name: Arc::clone(&method.name),
                params,
                ret,
                quantified,
            });
        }

        let id = DynAbility::hash_from_canonical(&def.name, &canonical);
        def.resolved_id = Some(id);

        // Record dependencies so `require_ability` pulls them transitively.
        if !dependencies.is_empty() {
            let registry = infer
                .ability_registry
                .get_or_insert_with(crate::types::AbilityRegistry::new);
            let mut info = crate::types::AbilityInfo::new(def.name.as_ref());
            for dep in &dependencies {
                info = info.with_dependency(*dep);
            }
            registry.register(id, info);
        }

        infer.ability_resolver.register_dynamic(DynAbility {
            id,
            name: Arc::clone(&def.name),
            methods,
            dependencies,
        });
        if let Some(ability) = infer.ability_resolver.get_dynamic(&def.name) {
            resolved.push(Arc::clone(ability));
        }
    }
    resolved
}

/// Resolve a module's `ability` declarations without checking the rest of
/// the module.
///
/// This is the entry point for **ability preludes**: an embedder parses a
/// module containing only `ability` declarations (e.g. the runtime
/// bindings interface), resolves them here, and registers the results as
/// namespaced dynamics on the resolver it threads into checking — and as
/// the identity/method-id source when binding host handlers on the VM.
///
/// Each declaration's `resolved_id` is written back into the AST, exactly
/// as during a full module check.
pub fn resolve_ability_declarations(
    module: &mut crate::ast::Module,
) -> (
    Vec<Arc<crate::ability_resolver::DynAbility>>,
    Vec<BoxedTypeError>,
) {
    let mut infer = Infer::new();
    let mut errors = Vec::new();
    let abilities = register_abilities(&mut infer, module, &mut errors);
    (abilities, errors)
}

/// Register the module's enum declarations and bring every visible
/// variant constructor into scope (prelude `Option`/`Result` plus locals;
/// locals shadow prelude variants of the same name).
fn register_enums(infer: &mut Infer, module: &crate::ast::Module, env: &mut TypeEnv) {
    for item in &module.items {
        if let crate::ast::ItemKind::Enum(enum_def) = &item.kind {
            infer.enum_registry.register_def(enum_def);
        }
    }

    // Synthetic binding ids for constructors, distinct from imports
    // (2_000_000+) and user bindings.
    let mut next_binding_id: BindingId = 4_000_000;
    let enums: Vec<_> = infer.enum_registry.iter().cloned().collect();
    for info in enums {
        for (idx, variant) in info.variants.iter().enumerate() {
            // Respect shadowing: only bind the variant if this enum is its
            // current owner.
            let owned = infer
                .enum_registry
                .resolve_variant(&variant.name)
                .is_some_and(|(owner, _)| owner.name == info.name);
            if !owned {
                continue;
            }
            let scheme = info.constructor_scheme(idx);
            env.insert(next_binding_id, Arc::clone(&variant.name), scheme);
            next_binding_id += 1;
        }
    }
}

/// Substitute type parameters in a type with type variables.
pub(super) fn substitute_type_params(
    ty: &Type,
    type_var_map: &HashMap<Arc<str>, TypeVarId>,
) -> Type {
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
    module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> CheckResult {
    check_module_core(Infer::new(), module, Some((module_path, registry)))
}

/// Check a single module with a custom ability resolver.
///
/// Like [`check_module`], but the resolver decides which abilities are
/// in scope (e.g. an embedder-registered runtime prelude).
#[must_use]
pub fn check_module_with_resolver(
    module: crate::ast::Module,
    resolver: AbilityResolver,
) -> CheckResult {
    check_module_core(Infer::with_resolver(resolver), module, None)
}

/// Check a module with cross-module support and a custom ability resolver.
///
/// This variant allows specifying which abilities are available at compile
/// time, which is useful for LSP and other tools that need to respect
/// package configuration.
#[must_use]
pub fn check_module_with_registry_and_resolver(
    module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    resolver: AbilityResolver,
) -> CheckResult {
    check_module_core(
        Infer::with_resolver(resolver),
        module,
        Some((module_path, registry)),
    )
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
                // Whole-module import (`use pkg.utils;` / `use core.list;`):
                // bind every public export under the qualified name
                // `<alias>.<export>`, which is how qualified expressions
                // look it up (see `ExprKind::Name` inference).
                if let Some(module_info) = registry.get(&target_path) {
                    for export in registry.get_public_exports(&target_path) {
                        if let Some(scheme) =
                            get_symbol_scheme(infer, &module_info.module, &export.name, export.kind)
                        {
                            let qualified: Arc<str> = format!("{name}.{}", export.name).into();
                            let binding_id = next_binding_id;
                            next_binding_id += 1;
                            env.insert(binding_id, qualified, scheme);
                        }
                    }
                }
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

    // Core modules are always in scope under their fully qualified names
    // (`core.list.map`), no import required.
    for module_info in registry.all_modules() {
        let path = module_info.path.clone();
        if !path.to_string().starts_with("core.") {
            continue;
        }
        for export in registry.get_public_exports(&path) {
            if let Some(scheme) =
                get_symbol_scheme(infer, &module_info.module, &export.name, export.kind)
            {
                let qualified: Arc<str> = format!("{path}.{}", export.name).into();
                let binding_id = next_binding_id;
                next_binding_id += 1;
                env.insert(binding_id, qualified, scheme);
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
                    // Foreign function: no ability inference — an absent
                    // `with` clause on an export means pure.
                    return Some(build_function_scheme(infer, func, false));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> crate::ast::Span {
        crate::ast::Span { start: 0, end: 0 }
    }

    fn method(
        name: &str,
        type_params: &[&str],
        params: &[(&str, Type)],
        ret_ty: Type,
    ) -> crate::ast::AbilityMethod {
        crate::ast::AbilityMethod {
            name: Arc::from(name),
            type_params: type_params
                .iter()
                .map(|name| crate::ast::TypeParam {
                    name: Arc::from(*name),
                    span: span(),
                })
                .collect(),
            params: params
                .iter()
                .map(|(name, ty)| (Arc::from(*name), ty.clone()))
                .collect(),
            ret_ty,
            span: span(),
        }
    }

    fn ability_module(name: &str, methods: Vec<crate::ast::AbilityMethod>) -> crate::ast::Module {
        crate::ast::Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![crate::ast::Item {
                kind: crate::ast::ItemKind::Ability(crate::ast::AbilityDef {
                    name: Arc::from(name),
                    name_span: span(),
                    dependencies: vec![],
                    methods,
                    resolved_id: None,
                }),
                span: span(),
                doc: None,
            }],
        }
    }

    /// A named type-parameter reference as the parser lowers it.
    fn ty_param(name: &str) -> Type {
        Type::Named(crate::types::NamedType::new(Arc::from(name), vec![]))
    }

    /// An in-language declaration must hash to the same identity as a
    /// descriptor-style rendering of the interface (method IDs are
    /// declaration indices, signatures render through the canonical type
    /// grammar): this is what lets host handlers keyed against the
    /// resolved declarations serve performs compiled from them.
    #[test]
    fn declaration_hashing_matches_descriptor_hashing() {
        use ambient_core::{hash_interface, MethodDescriptor};

        let mut module = ability_module(
            "Console",
            vec![
                method("print", &[], &[("message", Type::String)], Type::Unit),
                method("eprint", &[], &[("message", Type::String)], Type::Unit),
                method("println", &[], &[("message", Type::String)], Type::Unit),
            ],
        );

        let (abilities, errors) = resolve_ability_declarations(&mut module);
        assert!(errors.is_empty());
        assert_eq!(abilities.len(), 1);

        let expected = hash_interface(
            "Console",
            &[
                MethodDescriptor::new(0, "print", 1, |f| vec![f.string()], |f| f.unit()),
                MethodDescriptor::new(1, "eprint", 1, |f| vec![f.string()], |f| f.unit()),
                MethodDescriptor::new(2, "println", 1, |f| vec![f.string()], |f| f.unit()),
            ],
        );

        let console = &abilities[0];
        assert_eq!(console.id, expected);
        assert_eq!(console.method("print").map(|m| m.id), Some(0));
        assert_eq!(console.method("eprint").map(|m| m.id), Some(1));
        assert_eq!(console.method("println").map(|m| m.id), Some(2));

        // The identity is also written back for the compiler.
        let crate::ast::ItemKind::Ability(def) = &module.items[0].kind else {
            panic!("expected ability item");
        };
        assert_eq!(def.resolved_id, Some(console.id));
    }

    /// Generic methods are the risky parity case: the descriptor renders
    /// each `type_var()` occurrence as an independent `varN`, so the
    /// declaration must use a distinct type parameter per position
    /// (`run<T, R>` — never one parameter in two positions).
    #[test]
    fn generic_declaration_hashing_matches_descriptor_hashing() {
        use ambient_core::{hash_interface, MethodDescriptor};

        let list_of_string = Type::named("List", vec![Type::String]);
        let mut module = ability_module(
            "Execute",
            vec![
                method("has_function", &[], &[("hash", Type::String)], Type::Bool),
                method(
                    "get_dependencies",
                    &[],
                    &[("hash", Type::String)],
                    list_of_string.clone(),
                ),
                method(
                    "load_functions",
                    &[],
                    &[("bundle", Type::Bytes)],
                    Type::Unit,
                ),
                method(
                    "run",
                    &["T", "R"],
                    &[("hash", Type::String), ("args", ty_param("T"))],
                    ty_param("R"),
                ),
                method(
                    "get_functions",
                    &[],
                    &[("hashes", list_of_string)],
                    Type::Bytes,
                ),
                method(
                    "run_with",
                    &["T", "U", "R"],
                    &[
                        ("hash", Type::String),
                        ("args", ty_param("T")),
                        ("handler", ty_param("U")),
                    ],
                    ty_param("R"),
                ),
            ],
        );

        let (abilities, errors) = resolve_ability_declarations(&mut module);
        assert!(errors.is_empty());

        let expected = hash_interface(
            "Execute",
            &[
                MethodDescriptor::new(0, "has_function", 1, |f| vec![f.string()], |f| f.bool()),
                MethodDescriptor::new(
                    1,
                    "get_dependencies",
                    1,
                    |f| vec![f.string()],
                    |f| f.list(f.string()),
                ),
                MethodDescriptor::new(2, "load_functions", 1, |f| vec![f.bytes()], |f| f.unit()),
                MethodDescriptor::new(
                    3,
                    "run",
                    2,
                    |f| vec![f.string(), f.type_var()],
                    |f| f.type_var(),
                ),
                MethodDescriptor::new(
                    4,
                    "get_functions",
                    1,
                    |f| vec![f.list(f.string())],
                    |f| f.bytes(),
                ),
                MethodDescriptor::new(
                    5,
                    "run_with",
                    3,
                    |f| vec![f.string(), f.type_var(), f.type_var()],
                    |f| f.type_var(),
                ),
            ],
        );

        let execute = &abilities[0];
        assert_eq!(execute.id, expected);
        assert_eq!(execute.method("run").map(|m| m.id), Some(3));
        assert_eq!(execute.method("run_with").map(|m| m.id), Some(5));
    }
}
