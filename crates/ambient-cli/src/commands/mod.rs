//! CLI command implementations.
//!
//! Each command is implemented in its own submodule.

mod check;
mod compile;
mod dev;
pub(crate) mod host;
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

use ambient_engine::ability_resolver::{AbilityInterface, DynAbility};
use ambient_engine::ast::Module;
use ambient_engine::build::ParseFailure;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::diagnostic::print_diagnostic;

/// Parse source into an AST, converting a parse error into the engine's
/// renderable [`ParseFailure`].
///
/// This is the one bridge from `ambient_parser::ParseError` to the build
/// pipeline's parse-error currency, shared by every `build_package` caller
/// in the CLI so parse diagnostics render identically.
pub fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// The `platform` ability prelude: the bindings interface shipped by
/// `ambient-platform`, resolved to content-addressed identities.
///
/// Resolution is cheap (one small declaration module), and the resolved
/// types are `Rc`-based, so this is recomputed rather than cached in a
/// static.
pub fn platform_prelude() -> Result<Vec<Arc<DynAbility>>> {
    // A parse-only core registry supplies the prelude, so ability resolution
    // can seed the primitive nominals (`String`/`Number`/...) its signatures
    // hash against. Parsing the small core sources is cheap — nothing is
    // compiled — and keeps ability ids byte-stable through the module system.
    let mut registry = ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .map_err(|(module, e)| anyhow::anyhow!("core module `{module}` failed to parse: {e}"))?;

    let mut module = ambient_parser::parse(ambient_platform::ABILITY_DECLARATIONS)
        .map_err(|e| anyhow::anyhow!("platform bindings interface failed to parse: {e}"))?;
    let (abilities, errors) =
        ambient_engine::infer::resolve_ability_declarations(&mut module, &registry);
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

/// Core library context for single-file compilation: a registry with the
/// core modules registered, their compiled functions, and the
/// fully-qualified name→hash table user code links against.
pub struct CoreContext {
    pub registry: ModuleRegistry,
    pub compiled: CompiledModule,
    pub hashes: HashMap<ambient_engine::fqn::NameKey, blake3::Hash>,
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

    // Register the `core::system` declaration module so
    // `core::system::Network` resolves fully-qualified and
    // `use core::system::Network;` imports it.
    ambient_engine::core_library::register_declaration_module(
        &mut registry,
        &["core", "system"],
        ambient_platform::ABILITY_DECLARATIONS,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .map_err(|(module, e)| anyhow::anyhow!("platform module `{module}` failed to build: {e}"))?;

    let hashes = ambient_engine::build::linking_table(&module_function_hashes, &registry);

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

    // Type check with the core and platform modules visible. `core_context`
    // registered `platform`, so its namespaced abilities resolve through
    // registry seeding. The prelude is still needed below for the *compiler*
    // (host binding), a separate concern from type checking.
    let prelude = platform_prelude()?;
    let check_result =
        ambient_engine::infer::check_module_with_registry(module, &main_path, &core.registry);
    if !check_result.is_ok() {
        // Route type errors through the shared conversion so single-file
        // compile renders exactly what `ambient check` does.
        let diagnostics = ambient_analysis::type_error_diagnostics(&check_result.errors);
        for diagnostic in &diagnostics {
            print_diagnostic(source, file, diagnostic);
        }
        bail!(
            "Found {} type error(s) in {}",
            diagnostics.len(),
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
            env: ambient_engine::module_env::ModuleEnv::new(&core.registry, &main_path),
        },
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file.display()))?;

    compiled.merge(&core.compiled);
    Ok(compiled)
}
