//! CLI command implementations.
//!
//! Each command is implemented in its own submodule.

mod check;
mod compile;
mod dev;
mod host;
mod init;
mod run;
mod store;

pub use check::cmd_check;
pub use compile::cmd_compile;
pub use dev::cmd_dev;
pub use init::cmd_init;
pub use run::cmd_run;
pub use store::cmd_store;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use ambient_engine::ability_resolver::{
    AbilityInterface, AbilityResolver, DynAbility, core_abilities,
};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::diagnostic::print_diagnostic;

/// The `platform` ability prelude: the bindings interface shipped by
/// `ambient-platform`, resolved to content-addressed identities.
///
/// Resolution is cheap (one small declaration module), and the resolved
/// types are `Rc`-based, so this is recomputed rather than cached in a
/// static.
pub fn platform_prelude() -> Result<Vec<Arc<DynAbility>>> {
    let mut module = ambient_parser::parse(ambient_platform::ABILITY_DECLARATIONS)
        .map_err(|e| anyhow::anyhow!("platform bindings interface failed to parse: {e}"))?;
    let (abilities, errors) = ambient_engine::infer::resolve_ability_declarations(&mut module);
    if let Some(error) = errors.first() {
        bail!("platform bindings interface failed to resolve: {error}");
    }
    Ok(abilities)
}

/// The named ability's interface from the resolved platform prelude.
pub fn prelude_interface(prelude: &[Arc<DynAbility>], name: &str) -> Result<AbilityInterface> {
    prelude
        .iter()
        .find(|ability| ability.name.as_ref() == name)
        .map(|ability| AbilityInterface::from(&**ability))
        .ok_or_else(|| anyhow::anyhow!("platform prelude is missing the `{name}` ability"))
}

/// An ability resolver with the platform prelude registered under the
/// `platform` namespace, on top of the core abilities (Exception).
pub fn prelude_resolver(prelude: &[Arc<DynAbility>]) -> AbilityResolver {
    let mut resolver = core_abilities();
    for ability in prelude {
        resolver.register_dynamic_in_namespace("platform", (**ability).clone());
    }
    resolver
}

/// Core library context for single-file compilation: a registry with the
/// core modules registered, their compiled functions, and the
/// fully-qualified name→hash table user code links against.
pub struct CoreContext {
    pub registry: ModuleRegistry,
    pub compiled: CompiledModule,
    pub hashes: HashMap<Arc<str>, blake3::Hash>,
}

/// Build the core library context (used by check/compile/dev on bare
/// files; package builds do this inside the build pipeline).
pub fn core_context() -> Result<CoreContext> {
    let mut registry = ModuleRegistry::new();
    let mut module_function_hashes = HashMap::new();
    let compiled = ambient_engine::build::compile_core_modules(
        &mut registry,
        &mut module_function_hashes,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .map_err(|e| anyhow::anyhow!("core library failed to build: {e}"))?;

    let mut hashes = HashMap::new();
    for (path, module_hashes) in &module_function_hashes {
        for (name, hash) in module_hashes {
            hashes.insert(format!("{path}.{name}").into(), *hash);
        }
    }

    Ok(CoreContext {
        registry,
        compiled,
        hashes,
    })
}

/// Read source code from a file.
pub fn read_source(file: &Path) -> Result<String> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "ab" && ext != "ambient" {
        bail!("expected .ab source file, got: {}", file.display());
    }
    fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))
}

/// Compile source code to a module.
///
/// Single files compile against the core library: their `core.*` calls
/// type-check through the registry and link against the compiled core
/// functions, which are merged into the result.
pub fn compile_source(source: &str, file: &Path) -> Result<CompiledModule> {
    // Parse.
    let module = match ambient_parser::parse(source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    let mut core = core_context()?;
    let main_path = ModulePath::root();
    core.registry.register(&main_path, Arc::new(module.clone()));

    // Type check with the core modules and platform prelude visible.
    let prelude = platform_prelude()?;
    let check_result = ambient_engine::infer::check_module_with_registry_and_resolver(
        module,
        &main_path,
        &core.registry,
        prelude_resolver(&prelude),
    );
    if !check_result.is_ok() {
        // Print type errors
        for error in &check_result.errors {
            print_diagnostic(source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            check_result.errors.len(),
            file.display()
        );
    }

    // Compile the type-checked module with debug info, linking core.
    let source_file = file.display().to_string();
    let mut compiled = ambient_engine::compiler::compile_module_with_options(
        &check_result.module,
        ambient_engine::compiler::CompileOptions {
            source: Some(source),
            source_file: Some(&source_file),
            imported_hashes: Some(core.hashes),
            prelude_abilities: &prelude,
        },
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file.display()))?;

    compiled.merge(&core.compiled);
    Ok(compiled)
}
