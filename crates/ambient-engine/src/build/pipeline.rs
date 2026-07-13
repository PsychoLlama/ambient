//! The cold compile pipeline: the register → resolve → check → compile
//! machinery shared by `build_package` and the public core/declaration/REPL
//! entry points. Extracted from `build.rs` so the orchestrator (`mod.rs`) and
//! the incremental cache (`cache.rs`) each stay within the file-size budget.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ast::Module;
use crate::compiler::CompiledModule;
use crate::fqn::NameKey;
use crate::module_env::ModuleEnv;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::package::{LoadedModule, Package};

use super::cache::module_output;
use super::{BuildError, ModuleBuildOutput, ParseFn};

/// Load and parse every `.ab` file under the package's `src/` directory.
pub(super) fn load_all_modules(pkg: &mut Package, parse: ParseFn) -> Result<(), BuildError> {
    let mut paths = discover_module_paths(&pkg.src_path())
        .map_err(|e| BuildError::PackageOpen(format!("failed to scan src: {e}")))?;
    paths.sort_by_key(ToString::to_string);
    for module_path in paths {
        if module_path.collides_with_reserved_root() {
            return Err(BuildError::PackageOpen(format!(
                "module `{module_path}` collides with the reserved `{}` namespace; rename the file",
                module_path.segments()[0]
            )));
        }
        if pkg.is_loaded(&module_path) {
            continue;
        }
        let loaded = load_module(pkg, &module_path, parse)?;
        pkg.add_module(loaded);
    }
    Ok(())
}

/// Every module path under a source directory: each `.ab` file, mapped
/// through the canonical file↔module mapping.
///
/// # Errors
///
/// Returns an error if the directory tree cannot be read.
pub fn discover_module_paths(src: &Path) -> std::io::Result<Vec<ModulePath>> {
    let mut found = Vec::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("ab")
                && let Ok(relative) = path.strip_prefix(src)
                && let Some(module_path) = ModulePath::from_relative_file_path(relative)
            {
                found.push(module_path);
            }
        }
    }
    Ok(found)
}

/// Topologically order modules by their resolved dependencies
/// (dependencies first). Modules outside the package (core, platform) are
/// skipped.
///
/// The graph handed in is expected to be acyclic: package cycles are
/// rejected up front in [`build_package`] (see [`crate::module_cycles`]), and
/// the core/platform module groups are authored cycle-free. A cycle that
/// slipped through anyway would still terminate here — the `visited` guard
/// breaks the recursion — merely yielding an arbitrary order rather than
/// looping.
#[allow(clippy::items_after_statements)]
pub(super) fn compilation_order(deps: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut order = Vec::new();
    let mut visited = HashSet::new();

    fn visit(
        key: &str,
        deps: &BTreeMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if visited.contains(key) {
            return;
        }
        visited.insert(key.to_string());
        if let Some(module_deps) = deps.get(key) {
            for dep in module_deps {
                // Only package modules participate; core/platform compile
                // ahead of the package.
                if deps.contains_key(dep) {
                    visit(dep, deps, visited, order);
                }
            }
            order.push(key.to_string());
        }
    }

    for key in deps.keys() {
        visit(key, deps, &mut visited, &mut order);
    }
    order
}

/// The linking table for a module about to compile: every already-compiled
/// function, bound under its canonical name.
///
/// - Ordinary functions bind under their [`Fqn`] ([`NameKey::Item`]);
///   `core::collections::list::map`, `utils::helper`. The resolve pass
///   rewrites every cross-module reference to exactly this key.
/// - Impl-method dispatch symbols (`<uuid>::Trait::method`) are globally
///   unique content symbols and bind as-is under [`NameKey::Bare`], so
///   cross-module method calls link.
///
/// Module-local calls use the module's own bare names, which the compiler
/// seeds itself — they never pass through this table.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn linking_table(
    compiled_hashes: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    registry: &ModuleRegistry,
) -> HashMap<NameKey, blake3::Hash> {
    let mut table = HashMap::new();
    for (path, module_hashes) in compiled_hashes {
        for (name, hash) in module_hashes {
            // A content-addressed dispatch symbol keeps its bare identity;
            // an ordinary function keys under its `Fqn`.
            let key = if name.contains("::") {
                NameKey::Bare(Arc::clone(name))
            } else {
                NameKey::Item(registry.fqn(path, &[Arc::clone(name)]))
            };
            table.insert(key, *hash);
        }
    }
    table
}

/// Register and compile the embedded core library modules.
///
/// Core modules are registered in the registry under their reserved
/// `core::*` paths (so type checking can see them), compiled through the
/// ordinary pipeline, and their per-module function hashes are recorded
/// into `module_function_hashes` (so calls link).
///
/// Returns the merged compiled core module. The caller merges it into the
/// final build so core functions execute like any others.
///
/// # Errors
///
/// Returns an error if a core module fails to parse, check, or compile —
/// all of which are bugs in the embedded sources rather than user error.
#[allow(clippy::implicit_hasher)]
pub fn compile_core_modules(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    parse: impl Fn(&str) -> Result<Module, String>,
) -> Result<CompiledModule, BuildError> {
    let mut outputs = BTreeMap::new();
    compile_core_modules_collecting(registry, module_function_hashes, &mut outputs, parse)
}

/// Like [`compile_core_modules`], but also records each core module's
/// snapshot products into `outputs` (keyed by canonical identity). Used by
/// [`build_package`]; the public wrapper discards the collector.
#[allow(clippy::implicit_hasher)]
fn compile_core_modules_collecting(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    outputs: &mut BTreeMap<String, ModuleBuildOutput>,
    parse: impl Fn(&str) -> Result<Module, String>,
) -> Result<CompiledModule, BuildError> {
    let core_paths = crate::core_library::register_core_modules(registry, parse).map_err(
        |(module, error)| BuildError::Compile {
            module: format!("core.{module}"),
            error,
        },
    )?;

    // Compile the registered core modules in dependency order, reusing the
    // same resolve + topo-sort every module group shares.
    compile_module_group(registry, module_function_hashes, outputs, &core_paths)
}

/// Register and compile an embedder-supplied declaration *tree* (e.g. the
/// platform bindings interface: the `core::system` directory module plus
/// its per-ability submodules).
///
/// Each module is ordinary Ambient source — `unique(<uuid>)`-prefixed
/// ability declarations whose method bodies (the default implementations
/// unhandled performs run) call each module's own private `extern fn`s.
/// The tree checks and compiles exactly like the core library, in
/// dependency order; the caller must have merged the platform's native
/// bindings into the registry first, or the extern pre-pass fails loudly.
///
/// # Errors
///
/// Returns an error if a module fails to parse, check, or compile — bugs in
/// the embedder's interface, not user error.
#[allow(clippy::implicit_hasher)]
pub fn compile_declaration_modules(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    modules: &[crate::core_library::DeclModule<'_>],
    parse: impl Fn(&str) -> Result<Module, String>,
) -> Result<CompiledModule, BuildError> {
    let mut outputs = BTreeMap::new();
    compile_declaration_modules_collecting(
        registry,
        module_function_hashes,
        &mut outputs,
        modules,
        parse,
    )
}

/// Like [`compile_declaration_modules`], but also records each module's
/// snapshot products into `outputs`. Used by [`build_package`]; the public
/// wrapper discards the collector.
#[allow(clippy::implicit_hasher)]
fn compile_declaration_modules_collecting(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    outputs: &mut BTreeMap<String, ModuleBuildOutput>,
    modules: &[crate::core_library::DeclModule<'_>],
    parse: impl Fn(&str) -> Result<Module, String>,
) -> Result<CompiledModule, BuildError> {
    let paths = crate::core_library::register_declaration_modules(registry, modules, parse)
        .map_err(|(module, error)| BuildError::Compile { module, error })?;
    compile_module_group(registry, module_function_hashes, outputs, &paths)
}

/// Resolve, order, check, and compile an already-registered set of module
/// paths, recording their function hashes and returning the merged artifact.
///
/// Shared by [`compile_core_modules`] and [`compile_declaration_modules`]:
/// both register a group of reserved-path modules and then need the same
/// dependency-ordered compile as package modules get. Modules referenced
/// only as dependencies (e.g. a platform module performing `core::time`)
/// are already compiled and simply link through `module_function_hashes`.
#[allow(clippy::implicit_hasher, clippy::too_many_lines)]
pub(super) fn compile_module_group(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    outputs: &mut BTreeMap<String, ModuleBuildOutput>,
    paths: &[ModulePath],
) -> Result<CompiledModule, BuildError> {
    // Compile in dependency order (dependencies first), reusing the same
    // resolve + topo-sort as package modules rather than a hardcoded list.
    // Every module is registered, so resolving each canonicalizes its
    // cross-module references and yields its dependency set; the ASTs
    // themselves aren't rewritten here (the checker re-resolves, and
    // re-registering would drop the injected intrinsic exports).
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut paths_by_key: BTreeMap<String, ModulePath> = BTreeMap::new();
    // Resolve-pass dependency sets keyed and rendered by canonical module
    // identity, for the snapshot (matching `outputs`/`interfaces` keys).
    let mut dep_ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The raw resolve-pass dependency closures, keyed by path string, for
    // narrowing each module's `ModuleEnv` to what it can actually reference.
    let mut dep_closures: BTreeMap<String, std::collections::BTreeSet<crate::fqn::ModuleId>> =
        BTreeMap::new();
    for path in paths {
        // The prelude module is a pure re-export container (`pub use
        // core::option::{Some, None}`, ...). It is registered so its
        // re-exports can be injected into every scope, but it is never
        // itself checked or compiled: piecewise variant re-exports aren't a
        // normal importable surface, and it contributes no functions. No
        // module ever depends on it (injection resolves to each name's
        // origin), so leaving it out of the order is sound.
        if registry.prelude() == Some(path) {
            continue;
        }
        let mut ast = registry
            .get(path)
            .map(|info| (*info.module).clone())
            .ok_or_else(|| BuildError::PackageOpen(format!("module {path} vanished")))?;
        let outcome = crate::resolve::resolve_module(&mut ast, path, registry);
        deps.insert(
            path.to_string(),
            outcome.deps.iter().map(ToString::to_string).collect(),
        );
        dep_ids.insert(
            registry.module_id(path).to_string(),
            outcome.deps.iter().map(ToString::to_string).collect(),
        );
        dep_closures.insert(path.to_string(), outcome.deps);
        paths_by_key.insert(path.to_string(), path.clone());
    }

    // Check every module up front — checking reads only registered
    // signatures, so it is order-independent — then recover the compile-order
    // edges type-directed dispatch needs (an inherent-method or overloaded-
    // operator call links against another group module's compiled body, but
    // the reference is resolved by the checker, not the resolve pass, so it
    // never became a dependency edge above). See [`crate::dispatch_deps`].
    let mut checked: Vec<(String, crate::infer::CheckResult)> = Vec::new();
    for (key, path) in &paths_by_key {
        let ast = registry
            .get(path)
            .map(|info| info.module.clone())
            .ok_or_else(|| BuildError::PackageOpen(format!("module {path} vanished")))?;
        let check_result = crate::infer::check_module_with_registry((*ast).clone(), path, registry);
        if !check_result.is_ok() {
            // A reserved-path module failing to type-check is a compiler bug,
            // not user error, so there is no user source to render against.
            let joined = check_result
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BuildError::Compile {
                module: path.to_string(),
                error: joined,
            });
        }
        checked.push((key.clone(), check_result));
    }
    let modules_for_edges: Vec<(String, &crate::ast::Module)> = checked
        .iter()
        .map(|(key, cr)| (key.clone(), &cr.module))
        .collect();
    for (key, definers) in crate::dispatch_deps::dispatch_edges(&modules_for_edges) {
        let entry = deps.entry(key).or_default();
        for definer in definers {
            if !entry.contains(&definer) {
                entry.push(definer);
            }
        }
    }
    let mut checked_by_key: BTreeMap<String, crate::infer::CheckResult> =
        checked.into_iter().collect();

    let order = compilation_order(&deps);

    let mut merged = CompiledModule::new();
    for key in order {
        let path = paths_by_key
            .get(&key)
            .cloned()
            .ok_or_else(|| BuildError::PackageOpen(format!("module {key} vanished")))?;
        let check_result = checked_by_key
            .remove(&key)
            .ok_or_else(|| BuildError::PackageOpen(format!("module {key} vanished")))?;

        // The linking table this module compiles against — its `imported`
        // view. Also the snapshot's consumed-link witness (`module_output`
        // intersects it with the module's external references).
        let imported = linking_table(module_function_hashes, registry);
        let mut compiled = crate::compiler::compile_module_with_options(
            &check_result.module,
            crate::compiler::CompileOptions {
                imported_hashes: Some(imported.clone()),
                // These modules compile with the dependency-scoped view a
                // user module gets. In particular core modules construct
                // prelude enums (`collections/List.ab` builds bare
                // `Some`/`None`), which arrive via the prelude through
                // `resolve_imports` and record a dependency on the origin
                // module (`core::option`) — so the narrowed env still holds
                // their variants. Dispatch-only edges (bare type references
                // resolved by the checker) never feed these value-position
                // channels, so the resolve deps are the exact set.
                env: ModuleEnv::new_scoped(
                    registry,
                    &path,
                    dep_closures.get(&key).unwrap_or(&BTreeSet::new()),
                ),
                ..crate::compiler::CompileOptions::default()
            },
        )
        .map_err(|e| BuildError::Compile {
            module: path.to_string(),
            error: e.to_string(),
        })?;
        compiled.signatures = check_result.signatures;

        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }

        // Reserved-path modules share plain names (`list::map`, `option::map`,
        // ...). The merged artifact binds them fully qualified so they never
        // collide with each other or with user functions. Impl-method
        // dispatch symbols are already globally unique and carry their own
        // `::` (`List::all`), so they pass through unqualified — qualifying
        // them again would produce a double-qualified `core::collections::list::all`.
        compiled.function_names = qualify_names(&compiled.function_names, &path, registry);
        compiled.const_names = qualify_names(&compiled.const_names, &path, registry);
        compiled.signatures = qualify_names(&compiled.signatures, &path, registry);

        let module_id = registry.module_id(&path).to_string();
        // Builtin (core/platform) modules cache as one unit keyed by the
        // core-cache key, so their per-module `cache_key` is unused (zeros).
        outputs.insert(
            module_id.clone(),
            module_output(
                &compiled,
                dep_ids.get(&module_id).cloned().unwrap_or_default(),
                &imported,
                [0u8; 32],
            ),
        );

        module_function_hashes.insert(path, func_hashes);
        merged.merge(&compiled);
    }

    Ok(merged)
}

/// Load a single module from a package.
fn load_module(
    pkg: &Package,
    path: &ModulePath,
    parse: ParseFn,
) -> Result<LoadedModule, BuildError> {
    let source = pkg
        .read_module_source(path)
        .map_err(|e| BuildError::Compile {
            module: path.to_string(),
            error: format!("failed to read source: {e}"),
        })?;

    let file_path = pkg.module_file_path(path);
    let ast = parse(&source).map_err(|error| BuildError::Parse {
        module: path.to_string(),
        path: file_path,
        source: source.clone(),
        error: Box::new(error),
    })?;

    Ok(LoadedModule {
        path: path.clone(),
        source,
        ast,
    })
}

/// Compile a loaded module to bytecode with cross-module type checking.
///
/// `deps` is the module's resolve-pass dependency closure; the compiler's
/// foreign-item channels ([`ModuleEnv::new_scoped`]) are narrowed to it, so
/// the compile reads only the modules its cache key already folds.
pub(super) fn compile_loaded_module_with_registry(
    loaded: &LoadedModule,
    file_path: &Path,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
    deps: &std::collections::BTreeSet<crate::fqn::ModuleId>,
) -> Result<(CompiledModule, crate::compiler::PrelinkModule), BuildError> {
    let check_result =
        crate::infer::check_module_with_registry(loaded.ast.clone(), module_path, registry);

    if !check_result.is_ok() {
        return Err(BuildError::TypeCheck {
            module: module_path.to_string(),
            path: file_path.to_path_buf(),
            source: loaded.source.clone(),
            errors: check_result.errors,
        });
    }

    let source_file = file_path.display().to_string();
    let (mut compiled, mut prelink) = crate::compiler::compile_module_capturing(
        &check_result.module,
        crate::compiler::CompileOptions {
            source: Some(&loaded.source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            env: ModuleEnv::new_scoped(registry, module_path, deps),
        },
    )
    .map_err(|e| BuildError::Compile {
        module: module_path.to_string(),
        error: e.to_string(),
    })?;
    // Attach the checker signatures to both the runnable module and the
    // persisted symbolic form: a relink reconstructs `signatures` from the
    // prelink (the compiler never computes them), so the two must agree. Set
    // the prelink (borrows) before moving the map into the module.
    prelink.set_signatures(&check_result.signatures);
    compiled.signatures = check_result.signatures;

    Ok((compiled, prelink))
}

/// Check and compile a single in-memory session module against a registry,
/// then merge it onto a clone of a cached base module.
///
/// This is the REPL's per-turn pipeline. `base` is the already-built
/// core (+ project) module to merge onto; `imported_hashes` is the matching
/// [`NameKey`] linking table (`CoreContext::hashes` or
/// [`BuildResult::link_table`]) that resolves the session module's
/// cross-module calls. `registry` must already contain `module` (resolved)
/// plus every module it references. The session module's own function names
/// are qualified with `path` (`repl::foo`) before merging — matching how
/// [`build_package`] qualifies package modules — so the caller can deploy an
/// entry by its qualified name (`repl::__repl_entry_N`).
///
/// Mirrors [`compile_loaded_module_with_registry`] but keeps the "how to wire
/// the imported channels" logic here in the engine rather than duplicated in
/// the CLI frontend.
///
/// # Errors
///
/// Returns [`BuildError::TypeCheck`] if the module fails to type-check, or
/// [`BuildError::Compile`] if codegen fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_session_module(
    base: &CompiledModule,
    registry: &ModuleRegistry,
    module: &Module,
    path: &ModulePath,
    source: &str,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
) -> Result<CompiledModule, BuildError> {
    let check_result = crate::infer::check_module_with_registry(module.clone(), path, registry);

    if !check_result.is_ok() {
        return Err(BuildError::TypeCheck {
            module: path.to_string(),
            path: PathBuf::from(path.to_string()),
            source: source.to_string(),
            errors: check_result.errors,
        });
    }

    let source_file = path.to_string();
    let mut compiled = crate::compiler::compile_module_with_options(
        &check_result.module,
        crate::compiler::CompileOptions {
            source: Some(source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            env: ModuleEnv::new(registry, path),
        },
    )
    .map_err(|e| BuildError::Compile {
        module: path.to_string(),
        error: e.to_string(),
    })?;
    compiled.signatures = check_result.signatures;

    // Qualify this module's function and const names with its path
    // (`repl::foo`), the canonical identity, so deploy-by-name resolves them.
    // Impl-method dispatch symbols are already globally unique and pass through.
    compiled.function_names = qualify_names(&compiled.function_names, path, registry);
    compiled.const_names = qualify_names(&compiled.const_names, path, registry);
    compiled.signatures = qualify_names(&compiled.signatures, path, registry);

    let mut merged = base.clone();
    merged.merge(&compiled);
    Ok(merged)
}

/// Qualify a module's bare item names with its path (`gcd` → `math::gcd`),
/// the canonical identity, so store bindings never collide across modules.
/// Names already carrying `::` (impl-method dispatch symbols like
/// `<uuid>::Trait::method`, already globally unique) pass through untouched.
/// Applied identically to function, const, and signature maps.
pub(super) fn qualify_names<V: Clone>(
    names: &HashMap<Arc<str>, V>,
    path: &ModulePath,
    registry: &ModuleRegistry,
) -> HashMap<Arc<str>, V> {
    names
        .iter()
        .map(|(name, value)| {
            let qualified: Arc<str> = if name.contains("::") {
                Arc::clone(name)
            } else {
                registry.fqn(path, &[Arc::clone(name)]).to_string().into()
            };
            (qualified, value.clone())
        })
        .collect()
}
