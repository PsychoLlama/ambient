//! The final finalize-and-fold tail of a module compile, extracted so the
//! cold compile path ([`super::entry`]) and the incremental *relink* fast path
//! ([`super::prelink`]) run byte-for-byte the same code.
//!
//! Everything in [`AssembleInputs`] is the pre-finalization *symbolic* form of
//! a module: functions and lambdas still carry their temporary (name- or
//! counter-derived) hashes, and cross-module references carry the callee's
//! final hash *as observed when the module compiled*. Running
//! [`assemble_module`] on it is deterministic: [`finalize_module_hashes`]
//! groups the functions into content-addressed objects and derives every final
//! hash. The relink path persists exactly these inputs, remaps the moved
//! foreign hashes, and re-runs [`assemble_module`] — so a relinked module is
//! byte-identical to a cold recompile *by construction*, with no re-check and
//! no codegen.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::object::StoredObject;

use super::CompiledModule;
use super::error::{CompileError, CompileErrorKind};
use super::hash::finalize_module_hashes;
use super::module_output::MigrationRecord;

/// One ability default implementation's inputs to the post-finalize
/// "same-key" ambiguity check (which rejects two methods of an ability that
/// share a signature *and* an identical default implementation, because they
/// would collapse to one `MethodKey` at runtime).
///
/// Captured as owned data (not AST references) so both the cold compile and
/// the relink path — which has no AST — can run the check identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityMethodCheck {
    /// The method's dispatch symbol (`<ability-uuid>::<method>`); its final
    /// hash is looked up in the assembled module's `function_names`.
    pub symbol: Arc<str>,
    /// The ability's declaration uuid (a `MethodKey` input).
    pub uuid: uuid::Uuid,
    /// The method's canonical signature hash (`None` if the checker recorded
    /// none, in which case the method is skipped, exactly as in cold compile).
    pub signature: Option<ambient_core::SignatureHash>,
    /// The ability's name, for the diagnostic.
    pub ability_name: Arc<str>,
    /// The method's name, for the diagnostic.
    pub method_name: Arc<str>,
    /// The method's declaration span, for the diagnostic.
    pub span: (u32, u32),
}

/// The complete pre-finalization form of a module: the exact inputs
/// [`assemble_module`] folds into a finished [`CompiledModule`]. Both the cold
/// compiler and the relink path build one of these and hand it off.
pub struct AssembleInputs {
    /// Named/impl/ability functions: `(linking name, function, is_main)`.
    /// Each function's `hash` is its temporary (pre-finalization) hash.
    pub compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)>,
    /// Lambdas: `(temporary hash, parent name, function)`.
    pub lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
    /// Ability default-implementation symbol → rename-stable group name (the
    /// ability uuid), for canonical recursive-group ordering.
    pub ability_impl_group_names: HashMap<Arc<str>, Arc<str>>,
    /// Content-addressed `const` value objects (module-level and block-scoped),
    /// folded into the module's objects. Leaves; their hash is `object.hash()`.
    pub const_objects: Vec<StoredObject>,
    /// `const` name → value-object hash bindings.
    pub const_names: Vec<(Arc<str>, blake3::Hash)>,
    /// `extern fn` native objects, folded into the module's objects.
    pub native_objects: Vec<StoredObject>,
    /// `extern fn` name → native-object hash bindings.
    pub native_names: Vec<(Arc<str>, blake3::Hash)>,
    /// Static `State::init_versioned` migration obligations.
    pub migrations: Vec<MigrationRecord>,
    /// Ability-method ambiguity-check inputs.
    pub ability_checks: Vec<AbilityMethodCheck>,
}

/// Finalize content hashes and fold in the module's const/native objects and
/// migrations, then run the ability-method ambiguity check.
///
/// This is the single authority on how the symbolic form becomes a finished
/// [`CompiledModule`]; the cold compiler and the relink path both call it, so
/// their outputs can never drift. It does *not* attach `signatures` — those
/// come from the checker at a later seam, identically for both paths.
///
/// # Errors
///
/// Propagates [`finalize_module_hashes`] errors and reports the ability-method
/// ambiguity as an unsupported-feature error.
pub fn assemble_module(inputs: AssembleInputs) -> Result<CompiledModule, CompileError> {
    let AssembleInputs {
        compiled_functions,
        lambdas,
        ability_impl_group_names,
        const_objects,
        const_names,
        native_objects,
        native_names,
        migrations,
        ability_checks,
    } = inputs;

    let mut module =
        finalize_module_hashes(compiled_functions, lambdas, &ability_impl_group_names)?;
    module.migrations = migrations;
    // Fold in every const value object; a referencing function already records
    // the const hash in its dependencies.
    for object in const_objects {
        module.objects.entry(object.hash()).or_insert(object);
    }
    module.const_names = const_names.into_iter().collect();
    // Fold in the module's extern fns: they bind names and ship objects like
    // compiled functions.
    for object in native_objects {
        module.objects.entry(object.hash()).or_insert(object);
    }
    for (name, hash) in native_names {
        module.function_names.entry(name).or_insert(hash);
    }

    // A method's identity is (ability uuid, signature, implementation) — the
    // name is deliberately excluded. Two methods of one ability with the same
    // signature *and* an identical default implementation would derive one
    // `MethodKey`: performs and handler arms for them would be indistinguishable.
    // Reject the ambiguity now that final implementation hashes exist. This can
    // in principle first appear on a relink (a dependency edit collapsing two
    // callee hashes), so the check must run on both paths.
    let mut seen: HashMap<(uuid::Uuid, ambient_core::SignatureHash, blake3::Hash), Arc<str>> =
        HashMap::new();
    for check in &ability_checks {
        let (Some(signature), Some(impl_hash)) = (
            check.signature,
            module.function_names.get(check.symbol.as_ref()),
        ) else {
            continue;
        };
        if let Some(previous) = seen.insert(
            (check.uuid, signature, *impl_hash),
            Arc::clone(&check.method_name),
        ) {
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!(
                        "ability `{}` methods `{previous}` and `{}` share a signature \
                         and an identical default implementation, so they would be one \
                         method at runtime; make the implementations differ",
                        check.ability_name, check.method_name
                    ),
                },
                check.span,
            ));
        }
    }

    Ok(module)
}
