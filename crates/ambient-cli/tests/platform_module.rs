//! `platform` behaves like a first-class importable module root.
//!
//! `use platform::Console;` imports the ability as a bare name; a
//! fully-qualified `platform::Console` keeps working with no `use`; a bare,
//! never-imported ability is an error. The same bridge serves any
//! cross-module ability import (`use pkg::b::SomeAbility;`) — platform is
//! just its first consumer. These check through `check_module_with_registry`
//! (no embedder resolver), so they also pin the package-build resolver gap
//! as closed.

use std::sync::Arc;

use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

/// A registry with the core and platform modules registered, mirroring what
/// the CLI build paths assemble.
fn base_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules register");
    ambient_engine::core_library::register_declaration_module(
        &mut registry,
        &["platform"],
        ambient_platform::ABILITY_DECLARATIONS,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("platform module registers");
    registry
}

/// Type-check a single-file module (registered as root) and return its
/// error strings.
fn check_errors(source: &str) -> Vec<String> {
    let mut registry = base_registry();
    let module = ambient_parser::parse(source).expect("source parses");
    let main = ModulePath::root();
    registry.register(&main, Arc::new(module.clone()));
    ambient_engine::infer::check_module_with_registry(module, &main, &registry)
        .errors
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn use_platform_imports_ability_as_bare_name() {
    let errors = check_errors(
        "use platform::Console;\n\
         fn f(): () with Console { Console::print!(\"hi\") }",
    );
    assert!(errors.is_empty(), "bare use after import: {errors:?}");
}

#[test]
fn bare_never_imported_ability_is_an_error() {
    let errors = check_errors("fn f(): () with Console { Console::print!(\"hi\") }");
    assert!(
        !errors.is_empty(),
        "a bare, never-imported platform ability must not resolve"
    );
}

#[test]
fn fully_qualified_platform_needs_no_use() {
    // Backward compatible: `platform::Console` inline, no `use`.
    let errors =
        check_errors("fn f(): () with platform::Console { platform::Console::print!(\"hi\") }");
    assert!(errors.is_empty(), "fully-qualified, no use: {errors:?}");
}

#[test]
fn cross_module_user_ability_import() {
    // The general bridge: a user package ability imported by name from a
    // sibling module works bare, exactly like `platform`.
    let mut registry = base_registry();
    let b = ambient_parser::parse("pub ability Greet { fn hello(): (); }").expect("b parses");
    let b_path = ModulePath::from_str_segments(&["b"]).unwrap();
    registry.register(&b_path, Arc::new(b));

    let a_src = "use pkg::b::Greet;\n\
                 fn f(): () with Greet { Greet::hello!() }";
    let a = ambient_parser::parse(a_src).expect("a parses");
    let a_path = ModulePath::from_str_segments(&["a"]).unwrap();
    registry.register(&a_path, Arc::new(a.clone()));

    let errors: Vec<String> =
        ambient_engine::infer::check_module_with_registry(a, &a_path, &registry)
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
    assert!(errors.is_empty(), "cross-module ability import: {errors:?}");
}

#[test]
fn multi_module_package_uses_platform_qualified() {
    // A non-root module referencing `platform::Console` type-checks through
    // the registry path — the package-build resolver gap is closed.
    let mut registry = base_registry();
    let a_src = "pub fn shout(): () with platform::Console { platform::Console::print!(\"hi\") }";
    let a = ambient_parser::parse(a_src).expect("a parses");
    let a_path = ModulePath::from_str_segments(&["a"]).unwrap();
    registry.register(&a_path, Arc::new(a.clone()));

    let errors: Vec<String> =
        ambient_engine::infer::check_module_with_registry(a, &a_path, &registry)
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
    assert!(
        errors.is_empty(),
        "platform in a non-root module: {errors:?}"
    );
}

#[test]
fn local_ability_shadows_imported_platform() {
    // A local `ability Console` wins the bare name over the imported
    // platform one; the namespaced platform ability stays reachable
    // qualified. Both spellings type-check.
    let errors = check_errors(
        "use platform::Console;\n\
         ability Console { fn print(msg: string): (); }\n\
         fn local(): () with Console { Console::print!(\"hi\") }\n\
         fn plat(): () with platform::Console { platform::Console::print!(\"hi\") }",
    );
    assert!(errors.is_empty(), "local shadows imported: {errors:?}");
}
