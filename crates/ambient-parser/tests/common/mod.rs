//! Shared harness: check/compile a single module against the real core
//! library, with the prelude in scope.
//!
//! Option/Result and the primitive types reach a module through the
//! `core::prelude` injection now, not a hardcoded registry-less seed, so a
//! standalone `check_module` no longer sees them. These helpers stand in a
//! module up against a core-backed registry — exactly how `ambient
//! check`/`ambient run` build a package — so bare `Number`, `Some`, `None`,
//! `Option<T>`, etc. resolve.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::build::{
    build_foreign_const_hashes, build_foreign_enum_variants, build_foreign_unit_structs,
    build_imported_enums, compile_core_modules, linking_table,
};
use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::fqn::NameKey;
use ambient_engine::infer::{check_module_with_registry, resolve_registry_abilities};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

fn parse(source: &str) -> Result<ambient_engine::ast::Module, String> {
    ambient_parser::parse(source).map_err(|e| e.to_string())
}

/// A registry with the core library registered (and the prelude set), plus
/// the core link table for compilation.
struct Core {
    registry: ModuleRegistry,
    hashes: HashMap<NameKey, blake3::Hash>,
}

fn core() -> Core {
    let mut registry = ModuleRegistry::new();
    let mut module_function_hashes = HashMap::new();
    compile_core_modules(&mut registry, &mut module_function_hashes, parse)
        .expect("core library builds");
    let hashes = linking_table(&module_function_hashes, &registry);
    Core { registry, hashes }
}

/// Parse, type-check (asserting no errors), and compile `source` as the
/// root module against the core library.
pub fn compile(source: &str) -> CompiledModule {
    let module = ambient_parser::parse(source).expect("test source must parse");
    let mut core = core();
    let path = ModulePath::root();
    core.registry.register(&path, Arc::new(module.clone()));

    let checked = check_module_with_registry(module, &path, &core.registry);
    assert!(
        checked.errors.is_empty(),
        "type errors: {:?}",
        checked.errors
    );

    compile_module_with_options(
        &checked.module,
        CompileOptions {
            module_id: Some(core.registry.module_id(&path)),
            imported_hashes: Some(core.hashes.clone()),
            imported_enums: build_imported_enums(&path, &core.registry),
            imported_unit_structs: build_foreign_unit_structs(&path, &core.registry),
            imported_const_hashes: build_foreign_const_hashes(&path, &core.registry),
            foreign_enum_variants: build_foreign_enum_variants(&path, &core.registry),
            foreign_abilities: resolve_registry_abilities(&core.registry),
            ..CompileOptions::default()
        },
    )
    .expect("source should compile")
}
