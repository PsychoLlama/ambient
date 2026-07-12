//! Module-level type checking.
//!
//! The `check_module*` entry points drive a phased pipeline over one module
//! (see [`check_module_core`]); each phase lives in a submodule:
//!
//! 1. **Registration** — [`foreign`] registers the cross-module context
//!    (foreign package items, imports, the import env) and [`locals`]
//!    registers the module's own declarations (types, traits, enums,
//!    signatures) and validates declared types. Ability seeding and
//!    resolution live in [`abilities`].
//! 2. **Impl blocks** — [`impls`] checks trait and inherent impls and
//!    assigns dispatch symbols.
//! 3. **Function/const bodies** — [`bodies`] infers each body's type and
//!    effects.
//! 4. **Ability enforcement** — deferred checks recorded by [`bodies`] and
//!    [`impls`] run once all bodies are checked, driven from
//!    [`check_module_core`].

mod abilities;
mod ability_vars;
mod bodies;
mod foreign;
mod impls;
mod locals;
mod subst;
#[cfg(test)]
mod tests;

pub use abilities::{resolve_ability_declarations, resolve_registry_abilities};
pub(in crate::infer) use ability_vars::resolve_declared_with;
pub(in crate::infer) use locals::resolve_body_annotation;
pub(in crate::infer) use subst::{substitute_named, substitute_type_params};

use crate::ability_resolver::AbilityResolver;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::types::AbilitySet;

use super::Infer;
use super::env::TypeEnv;
use super::error::{BoxedTypeError, TypeError, TypeErrorKind};

use bodies::{
    DeferredAbilityCheck, check_ability_method_bodies, check_const_body, check_function_body,
    enforce_ability_subset, enforce_declared_abilities,
};
use foreign::register_cross_module;
use impls::check_impls;
use locals::register_local_declarations;

/// Result of type checking a module.
#[derive(Debug)]
pub struct CheckResult {
    /// Type errors found during checking.
    pub errors: Vec<BoxedTypeError>,
    /// The typed module (with types filled in on expressions).
    pub module: crate::ast::Module,
    /// Canonical type signature of every named item (function, extern fn,
    /// const), keyed by its bare name: the checked scheme with the final
    /// substitution applied, rendered by [`CanonicalTypeRenderer`]. This is
    /// the signature half of a deploy generation's name bindings
    /// (`Fqn → (hash, canonical signature)`, see `ref/live-upgrade.md`) —
    /// the rebinding rule compares these strings for equality.
    pub signatures: std::collections::HashMap<std::sync::Arc<str>, std::sync::Arc<str>>,
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
/// in scope (e.g. an embedder-registered platform prelude).
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

    // Seed the workspace package name so any `ModuleId`/`Fqn` the checker
    // mints (ability annotation namespaces, imported type-alias keys)
    // matches the ones the registry and resolve pass produce.
    if let Some((_, registry)) = cross_module {
        infer.set_workspace_name(registry.workspace_name().clone());
    }

    // The current module's identity: the key its own items bind under and
    // its same-module refs resolve to; `None` registry-less (own items bare).
    let current_module_id = cross_module.map(|(path, reg)| reg.module_id(path));

    // Phase 0: canonicalize cross-module references. Every import, module
    // alias, and inline rooted path is resolved to its one fully-qualified
    // identity (`QualifiedName::resolved`); everything downstream keys off
    // that. Module-level import failures are reported from the module
    // scope in `build_import_env`; block-scoped `use` failures only exist
    // here.
    if let Some((module_path, registry)) = cross_module {
        let outcome = crate::resolve::resolve_module(&mut module, module_path, registry);
        for failed in outcome.errors {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::ImportFailed {
                    message: failed.error.to_string(),
                },
                (failed.span.start, failed.span.end),
            )));
        }
    }

    // Phase 1: registration.
    let mut env = match cross_module {
        Some((module_path, registry)) => {
            register_cross_module(&mut infer, module_path, registry, &mut errors)
        }
        None => TypeEnv::new(),
    };

    register_local_declarations(
        &mut infer,
        &mut module,
        &mut env,
        &mut errors,
        current_module_id.as_ref(),
    );

    // Now that every ability — local and (in a registry) foreign — is
    // registered with its resolved dependencies, reject `with` cycles: the
    // method-key hash folds in each dependency's default implementation, so
    // the dependency graph must be acyclic.
    abilities::check_ability_dependency_cycles(
        &module,
        current_module_id.as_ref(),
        cross_module.map(|(_, registry)| registry),
        &mut errors,
    );

    // Phase 2: impl blocks. Inherent method bodies record their inferred
    // abilities for deferred enforcement (like functions, phase 4).
    let mut deferred_method_abilities: Vec<DeferredAbilityCheck> = Vec::new();
    check_impls(
        &mut infer,
        &mut module,
        &env,
        &mut errors,
        &mut deferred_method_abilities,
    );

    // Phase 3: function and const bodies.
    // Records (item index, raw inferred abilities) for deferred enforcement.
    let mut inferred_abilities: Vec<(usize, AbilitySet)> = Vec::new();

    for (idx, item) in module.items.iter_mut().enumerate() {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            check_function_body(
                &mut infer,
                func,
                idx,
                &env,
                current_module_id.as_ref(),
                &mut errors,
                &mut inferred_abilities,
            );
        }

        if let crate::ast::ItemKind::Const(const_def) = &mut item.kind {
            check_const_body(&mut infer, &env, const_def, &mut errors);
        }

        // Ability default implementations check like inherent methods:
        // bodies now, declared-dependency (effect-row) enforcement deferred
        // to phase 4.
        if let crate::ast::ItemKind::Ability(def) = &mut item.kind {
            check_ability_method_bodies(
                &mut infer,
                def,
                &env,
                &mut errors,
                &mut deferred_method_abilities,
            );
        }
    }

    // Phase 4: enforce declared abilities with final substitutions applied.
    // Handle expressions whose body effects were polymorphic at the handle
    // site resolve first, so their remainders are concrete for enforcement;
    // deferred sandbox restrictions run on the resolved sets.
    infer.resolve_pending_discharges();
    infer.resolve_pending_sandbox_checks();
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
    for check in &deferred_method_abilities {
        enforce_ability_subset(
            &infer,
            &check.context,
            &check.declared,
            &check.inferred,
            (check.span.start, check.span.end),
            &mut errors,
        );
    }

    errors.extend(infer.take_pending_errors());

    // Every body is checked and the substitution is final: render each named
    // item's canonical signature (an unannotated private function's inferred
    // types are resolved by now, so the rendering reflects the checked type,
    // not the pre-inference placeholder vars).
    let signatures = render_item_signatures(&infer, &module, &env, current_module_id.as_ref());

    CheckResult {
        errors,
        module,
        signatures,
    }
}

/// Render the canonical type signature of every named item from its checked
/// scheme, final substitution applied.
///
/// The rendering reuses [`CanonicalTypeRenderer`] — the same authority
/// ability method signatures hash through — so there is exactly one canonical
/// type encoding: variables number by first occurrence (deterministic across
/// compiles), nominal heads render by uuid (rename-stable), abilities render
/// by sorted id. Trait bounds are interface, so they enter the rendering as a
/// canonical `where` suffix in dictionary order (`crate::ast::dict_params`'s
/// order, which [`Scheme::bounds`](super::env::Scheme) preserves), each bound
/// keyed by its variable's occurrence number and the trait's nominal uuid.
fn render_item_signatures(
    infer: &Infer,
    module: &crate::ast::Module,
    env: &TypeEnv,
    module_id: Option<&crate::fqn::ModuleId>,
) -> std::collections::HashMap<std::sync::Arc<str>, std::sync::Arc<str>> {
    use crate::ability_resolver::CanonicalTypeRenderer;
    use std::fmt::Write;

    let mut signatures = std::collections::HashMap::new();
    for item in &module.items {
        let name = match &item.kind {
            crate::ast::ItemKind::Function(f) => &f.name,
            crate::ast::ItemKind::ExternFn(f) => &f.name,
            crate::ast::ItemKind::Const(c) => &c.name,
            _ => continue,
        };
        let Some(scheme) = locals::own_item_scheme(env, module_id, name) else {
            continue;
        };
        let ty = infer.apply(&scheme.ty);
        let mut renderer = CanonicalTypeRenderer::new();
        let mut sig = renderer.render(&ty);
        for (var, bound) in &scheme.bounds {
            let var = renderer.render(&crate::types::Type::Var(*var));
            let _ = write!(sig, " where {var}: {}", bound.trait_uuid);
        }
        signatures.insert(std::sync::Arc::clone(name), std::sync::Arc::from(sig));
    }
    signatures
}
