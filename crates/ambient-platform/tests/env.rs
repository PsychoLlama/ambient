//! Integration tests for the `Env` natives.
//!
//! These compile real Ambient source against a compiled `core::system`
//! module and drive it through a VM — the same path a compiled
//! `core::system::Env::args!()` perform takes: unhandled perform → the
//! ability's default implementation → the `env_*` native bound on the VM.

use std::sync::Arc;

use ambient_engine::compiler::CompiledModule;
use ambient_engine::value::Value;
use ambient_engine::vm::Vm;

/// Compile `src` against core + the compiled `core::system` module,
/// returning the merged deployable module.
fn compile(src: &str) -> CompiledModule {
    let module = ambient_parser::parse(src).expect("test program parses");

    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    let mut module_function_hashes = std::collections::HashMap::new();
    let core_compiled = ambient_engine::build::compile_core_modules(
        &mut registry,
        &mut module_function_hashes,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("core modules compile");

    // The build-time native contract needs every platform extern bound.
    registry
        .natives_mut()
        .merge(&ambient_platform::stub_natives());
    let platform_compiled = ambient_engine::build::compile_system_module(
        &mut registry,
        &mut module_function_hashes,
        ambient_platform::PLATFORM_SOURCE,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("core::system compiles");

    let path = ambient_engine::module_path::ModulePath::root();
    registry.register(&path, Arc::new(module.clone()));

    let checked = ambient_engine::infer::check_module_with_registry(module, &path, &registry);
    assert!(
        checked.is_ok(),
        "test program type-checks: {:?}",
        checked
            .errors
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
    );
    let compiled = ambient_engine::compiler::compile_module_with_options(
        &checked.module,
        ambient_engine::compiler::CompileOptions {
            source: Some(src),
            source_file: None,
            imported_hashes: Some(ambient_engine::build::linking_table(
                &module_function_hashes,
                &registry,
            )),
            env: ambient_engine::module_env::ModuleEnv::new(&registry, &path),
        },
    )
    .expect("test program compiles");

    let mut merged = core_compiled;
    merged.merge(&platform_compiled);
    merged.merge(&compiled);
    merged
}

/// Load a compiled module into a fresh VM and call its `run`.
fn run(compiled: &CompiledModule, natives: &ambient_engine::natives::NativeRegistry) -> Value {
    let mut vm = Vm::new();
    vm.register_natives(natives);
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
        if let Some((uuid, param_count)) = object.as_native() {
            vm.load_native(*hash, uuid, param_count);
        }
    }
    // Exact-name lookup only: ability default implementations compile
    // under `<uuid>::<method>` symbols, so `Execute::run`'s impl also ends
    // in `::run` — a suffix match would hit it.
    let entry = compiled
        .function_names
        .get("run")
        .copied()
        .or(compiled.entry_point)
        .expect("program has a run entry");
    vm.call(&entry, vec![]).expect("perform succeeds")
}

#[test]
fn args_returns_the_captured_argv() {
    let compiled = compile(
        r"
        pub fn run(): List<String> with core::system::Env {
          core::system::Env::args!()
        }
        ",
    );

    let argv = Arc::new(vec!["prog".to_string(), "a".to_string(), "b".to_string()]);
    let result = run(&compiled, &ambient_platform::env_natives(argv));
    assert_eq!(
        result,
        Value::list(vec![
            Value::string("prog"),
            Value::string("a"),
            Value::string("b"),
        ]),
        "args() returns the captured argv, program path first"
    );
}

#[test]
fn pid_returns_the_process_id() {
    let compiled = compile(
        r"
        pub fn run(): Number with core::system::Env {
          core::system::Env::pid!()
        }
        ",
    );

    let result = run(
        &compiled,
        &ambient_platform::env_natives(Arc::new(Vec::new())),
    );
    match result {
        Value::Number(n) => {
            assert_eq!(n, f64::from(std::process::id()), "pid matches the OS pid");
        }
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn cwd_returns_a_non_empty_string() {
    let compiled = compile(
        r#"
        pub fn run(): String with core::system::Env {
          core::system::Env::cwd!().unwrap_or("")
        }
        "#,
    );

    let result = run(
        &compiled,
        &ambient_platform::env_natives(Arc::new(Vec::new())),
    );
    match result {
        Value::String(s) => assert!(!s.is_empty(), "cwd is a non-empty path"),
        other => panic!("expected a string, got {other:?}"),
    }
}
