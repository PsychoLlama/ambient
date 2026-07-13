//! Cross-frontend parity of *builtin* (core + platform) module interfaces.
//!
//! The incremental-compilation cache keys a module on its dependencies'
//! `interface_hash`es, so a builtin's interface hash is a cache-key input for
//! every module that imports it. Two frontends derive these interfaces — the
//! compiler (`ambient_engine::build::build_package`) and the editor
//! (`ambient_analysis`) — and they must agree byte-for-byte, or a build
//! snapshot could never warm-start analysis (and vice versa).
//!
//! The subtlety this pins: a builtin's interface is read off its *registered*
//! AST, and that AST can be in raw (as-parsed) or resolved (canonicalized)
//! form. User modules are always registered resolved; builtins used to be left
//! raw. Raw renders a cross-module type/ability reference in its spelled form
//! (bare `Stdio`) instead of its canonical `Fqn`, so the two forms hash
//! differently for nearly every core module. Both frontends now re-register
//! builtins resolved (`core_library::resolve_builtin_modules`), so:
//!
//! 1. engine builtin interfaces == analysis builtin interfaces (the
//!    cross-frontend lock), and
//! 2. both equal the resolved-form reference (the "resolved is honest" lock —
//!    this leg fails if a frontend regresses to deriving from raw ASTs).

use std::collections::BTreeMap;
use std::fs;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, ParseFailure, build_package};
use ambient_engine::module_interface::{ModuleInterface, build_interfaces};
use ambient_engine::module_registry::ModuleRegistry;
use tempfile::TempDir;

fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// A minimal on-disk package so both frontends can open the same thing.
fn trivial_package() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"test_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("src");
    fs::write(src.join("main.ab"), "pub fn run(): Number { 1 }\n").expect("main");
    dir
}

/// The resolved-form reference: register core + platform raw, resolve every
/// builtin, and derive interfaces — independent of either production frontend.
fn resolved_reference() -> BTreeMap<String, blake3::Hash> {
    let mut registry = ModuleRegistry::new();
    let parse = |s: &str| ambient_parser::parse(s).map_err(|e| e.to_string());
    let core = ambient_engine::core_library::register_core_modules(&mut registry, parse)
        .expect("core registers");
    let platform = ambient_engine::core_library::register_declaration_modules(
        &mut registry,
        ambient_platform::platform_modules(),
        parse,
    )
    .expect("platform registers");
    let builtins: Vec<_> = core.into_iter().chain(platform).collect();
    ambient_engine::core_library::resolve_builtin_modules(&mut registry, &builtins);
    build_interfaces(&registry)
        .into_iter()
        .filter(|(k, _)| is_builtin(k))
        .map(|(k, s)| (k, s.interface_hash))
        .collect()
}

fn is_builtin(module_id: &str) -> bool {
    // Builtin scope renders under the reserved `core` root (`core`,
    // `core::option`, `core::system::stdio`, …); user modules render
    // `workspace::…`. Guard against a `workspace` pkg literally named `core*`.
    module_id == "core" || module_id.starts_with("core::")
}

#[test]
fn engine_and_analysis_derive_identical_builtin_interfaces() {
    let dir = trivial_package();

    // Engine: the compiler's view, with the platform bindings + stub natives
    // the real CLI build wires in.
    let stubs = ambient_platform::stub_natives();
    let engine = build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    )
    .expect("build");

    // Analysis: the editor's view of the same package.
    let analysis = ambient_analysis::package::AnalysisPackage::open(dir.path())
        .expect("open package")
        .module_interfaces();

    let reference = resolved_reference();
    assert!(
        !reference.is_empty(),
        "reference must cover the builtin modules"
    );

    let mut compared = 0usize;
    for (key, esum) in &engine.interfaces {
        if !is_builtin(key) {
            continue;
        }
        let asum = analysis
            .get(key)
            .unwrap_or_else(|| panic!("analysis is missing builtin module `{key}`"));

        // (1) Cross-frontend lock: the two production frontends must agree on
        // both the observable interface and the whole-module resolved AST hash.
        assert_eq!(
            esum.interface_hash, asum.interface_hash,
            "interface hash diverges between engine and analysis for `{key}`"
        );
        assert_eq!(
            esum.interface.encode(),
            asum.interface.encode(),
            "interface encoding diverges between engine and analysis for `{key}`"
        );
        assert_eq!(
            esum.resolved_ast_hash, asum.resolved_ast_hash,
            "resolved AST hash diverges between engine and analysis for `{key}`"
        );

        // (2) Resolved-is-honest lock: both must equal the resolved-form
        // reference. This leg fails if a frontend derives builtin interfaces
        // from raw ASTs.
        let want = reference
            .get(key)
            .unwrap_or_else(|| panic!("reference is missing builtin module `{key}`"));
        assert_eq!(
            esum.interface_hash, *want,
            "engine builtin interface for `{key}` is not the resolved form"
        );
        compared += 1;
    }

    assert!(
        compared >= 30,
        "expected to compare the full core+platform set, only saw {compared}"
    );
}

/// Guard the helper the fix relies on: resolving the builtins actually moves
/// most interface hashes, so the parity above is not vacuously satisfied by
/// raw == resolved.
#[test]
fn resolving_builtins_changes_their_interfaces() {
    let mut raw = ModuleRegistry::new();
    let parse = |s: &str| ambient_parser::parse(s).map_err(|e| e.to_string());
    let core = ambient_engine::core_library::register_core_modules(&mut raw, parse)
        .expect("core registers");
    let platform = ambient_engine::core_library::register_declaration_modules(
        &mut raw,
        ambient_platform::platform_modules(),
        parse,
    )
    .expect("platform registers");
    let paths: Vec<_> = core.into_iter().chain(platform).collect();

    let raw_hashes: BTreeMap<String, blake3::Hash> = paths
        .iter()
        .map(|p| {
            (
                raw.module_id(p).to_string(),
                ModuleInterface::from_module(&raw, p).interface_hash(),
            )
        })
        .collect();

    let reference = resolved_reference();
    let changed = raw_hashes
        .iter()
        .filter(|(k, h)| reference.get(*k).is_some_and(|r| r != *h))
        .count();
    assert!(
        changed > 10,
        "resolving builtins should move many interface hashes, moved {changed}"
    );
}
