//! Regression coverage for the compiler-facing [`ModuleEnv`] channel.
//!
//! `ModuleEnv` is the single derivation of "what does this module see?", so
//! its foreign-item sets must agree with the checker's. These tests pin that
//! agreement against a real core-backed registry.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::build::compile_core_modules;
use ambient_engine::fqn::NameKey;
use ambient_engine::module_env::ModuleEnv;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

fn parse(source: &str) -> Result<ambient_engine::ast::Module, String> {
    ambient_parser::parse(source).map_err(|e| e.to_string())
}

/// A registry with the full core library compiled and registered.
fn core_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    let mut module_function_hashes = HashMap::new();
    compile_core_modules(&mut registry, &mut module_function_hashes, parse)
        .expect("core library builds");
    registry
}

/// The primitives (`Number`, `String`, ...) and containers (`List`, `Map`,
/// `Set`) are `extern` unit structs: unit *shape* but never a value. They
/// must not leak into the value-only `foreign_unit_structs` channel — the
/// checker rejects them as values via `is_unit_value()`, and a `ModuleEnv`
/// that disagreed would be exactly the checker/compiler drift the invariant
/// forbids. A genuine (non-`extern`) user unit struct must still appear.
#[test]
fn extern_unit_structs_are_not_value_channel_entries() {
    let mut registry = core_registry();

    let shapes_path = ModulePath::from_str_segments(&["shapes"]).unwrap();
    let shapes = parse("pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5DEE) struct Origin;")
        .expect("shapes parses");
    registry.register(&shapes_path, Arc::new(shapes));

    let main_path = ModulePath::root();
    registry.register(&main_path, Arc::new(ambient_engine::ast::Module::default()));

    let env = ModuleEnv::new(&registry, &main_path);

    for key in &env.foreign_unit_structs {
        let rendered = key.to_string();
        assert!(
            !rendered.contains("core::primitives") && !rendered.contains("core::collections"),
            "extern unit struct leaked into the value channel: {rendered}"
        );
    }

    let origin = NameKey::Item(registry.fqn(&shapes_path, &[Arc::from("Origin")]));
    assert!(
        env.foreign_unit_structs.contains(&origin),
        "a genuine user unit struct must appear in foreign_unit_structs; got {:?}",
        env.foreign_unit_structs
    );
}
