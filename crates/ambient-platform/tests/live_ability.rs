//! End-to-end tests for the `Live` ability (see ref/live-upgrade.md).
//!
//! Each test compiles real Ambient source against core + `core::system`
//! and exercises `Live::latest!` through the deploy core: old code (an
//! already-deployed hash) resolves a ref forward across deploys, while a
//! VM with no deploy runtime — plain `ambient run`'s stub — behaves as
//! identity.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployRuntime, functions_from_module};

/// Compile a test program against core + compiled `core::system` — the
/// same world `ambient run` checks against (the `Live` default
/// implementation links by hash, so its compiled functions must merge in).
fn compile(src: &str) -> CompiledModule {
    let module = ambient_parser::parse(src).expect("test program parses");

    let mut registry = ModuleRegistry::new();
    let mut module_function_hashes = HashMap::new();
    let core_compiled = ambient_engine::build::compile_core_modules(
        &mut registry,
        &mut module_function_hashes,
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("core modules compile");
    registry
        .natives_mut()
        .merge(&ambient_platform::stub_natives());
    let platform_compiled = ambient_engine::build::compile_declaration_modules(
        &mut registry,
        &mut module_function_hashes,
        ambient_platform::platform_modules(),
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("core::system compiles");
    let path = ModulePath::root();
    registry.register(&path, Arc::new(module.clone()));

    let checked = check_module_with_registry(module, &path, &registry);
    assert!(
        checked.is_ok(),
        "test program type-checks: {:?}",
        checked
            .errors
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
    );
    let mut compiled = compile_module_with_options(
        &checked.module,
        CompileOptions {
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
    compiled.signatures = checked.signatures.clone();

    let mut merged = core_compiled;
    merged.merge(&platform_compiled);
    merged.merge(&compiled);
    merged
}

/// A deploy runtime whose VMs carry the platform stubs — the extern
/// contract every VM needs; the runtime's own `build_vm` overlays the
/// real `live_latest` resolution.
fn runtime() -> DeployRuntime {
    DeployRuntime::new(Arc::new(|| {
        let mut vm = Vm::new();
        vm.register_natives(&ambient_platform::stub_natives());
        vm
    }))
}

fn named_hash(compiled: &CompiledModule, name: &str) -> blake3::Hash {
    *compiled
        .function_names
        .get(name)
        .unwrap_or_else(|| panic!("test program defines `{name}`"))
}

const V1: &str = r"
    pub fn run(): () { }
    pub fn target(): Number { 1 }
    pub fn use_latest(): Number with core::system::Live {
      let f = core::system::Live::latest!(target);
      f()
    }
    ";

/// The core live-upgrade guarantee: code compiled and deployed in
/// generation 1 — its hash unchanged, its compile-time ref pinned to the
/// old target — observes new behavior after a deploy rebinds the target's
/// name, because its `latest!` read follows the name.
#[test]
fn old_code_picks_up_new_behavior_through_latest() {
    // v2 changes only `target`'s body; same canonical signature.
    let v2 = V1.replace("{ 1 }", "{ 2 }");

    let v1 = compile(V1);
    let v2 = compile(&v2);
    let use_latest_v1 = named_hash(&v1, "use_latest");
    assert_ne!(
        use_latest_v1,
        named_hash(&v2, "use_latest"),
        "v2's use_latest embeds the new target ref, so its hash moves — \
         the test calls the *old* hash on purpose"
    );

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &named_hash(&v1, "run"), |_| {})
        .expect("v1 deploys");
    let mut vm = core.build_vm();
    let before = vm.call(&use_latest_v1, Vec::new()).expect("v1 code runs");
    assert_eq!(format!("{before:?}"), "Number(1.0)");

    core.deploy(&functions_from_module(&v2), &named_hash(&v2, "run"), |_| {})
        .expect("v2 deploys");

    // Same old hash, fresh VM: the `latest!` read resolves target's name
    // to its current binding.
    let mut vm = core.build_vm();
    let after = vm
        .call(&use_latest_v1, Vec::new())
        .expect("old code still runs");
    assert_eq!(
        format!("{after:?}"),
        "Number(2.0)",
        "old code must observe the rebound target through Live::latest!"
    );
}

/// The rebinding rule, end to end: a target whose signature changed is
/// retire-and-fresh, so old code's `latest!` keeps resolving its old ref
/// to itself and behavior does not drift.
#[test]
fn signature_changed_target_stays_pinned_for_old_code() {
    let v2 = r#"
        pub fn run(): () { }
        pub fn target(tag: String): Number { 2 }
        pub fn use_latest(): Number with core::system::Live {
          let f = core::system::Live::latest!(target);
          f("ignored")
        }
        "#;

    let v1 = compile(V1);
    let v2 = compile(v2);
    let use_latest_v1 = named_hash(&v1, "use_latest");

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &named_hash(&v1, "run"), |_| {})
        .expect("v1 deploys");
    let report = core
        .deploy(&functions_from_module(&v2), &named_hash(&v2, "run"), |_| {})
        .expect("v2 deploys");
    assert!(
        report.names.retired.contains(&Arc::from("target")),
        "the signature change must report as retire-and-fresh: {:?}",
        report.names
    );

    // Old code still calls the old target — no arity mismatch, no drift.
    let mut vm = core.build_vm();
    let value = vm
        .call(&use_latest_v1, Vec::new())
        .expect("old code still runs against its pinned target");
    assert_eq!(format!("{value:?}"), "Number(1.0)");
}

/// Identity under plain `ambient run`: with no deploy runtime at all —
/// the stub is the wired implementation — `Live::latest!` resolves every
/// ref to itself and the program behaves like the pinned one.
#[test]
fn latest_is_identity_without_a_deploy_runtime() {
    let v1 = compile(V1);

    // A bare VM: platform stubs only, the module loaded by hand. This is
    // the compile-only/no-runtime world — no DeployRuntime, no resolver.
    let mut vm = Vm::new();
    vm.register_natives(&ambient_platform::stub_natives());
    let generation = functions_from_module(&v1);
    for func in generation.functions.values() {
        vm.load_function_shared(Arc::clone(func));
    }
    for (hash, value) in &generation.values {
        vm.load_value(*hash, value.clone());
    }
    for (hash, (uuid, param_count)) in &generation.natives {
        vm.load_native(*hash, *uuid, *param_count);
    }

    let value = vm
        .call(&named_hash(&v1, "use_latest"), Vec::new())
        .expect("the identity stub serves latest!");
    assert_eq!(format!("{value:?}"), "Number(1.0)");
}

/// The runtime enforces `latest`'s function-shaped contract (its
/// parameter is a bare generic): a non-function argument raises a
/// catchable exception rather than resolving.
#[test]
fn latest_rejects_non_functions() {
    let src = r#"
        pub fn run(): () { }
        pub fn misuse(): Number with core::system::Live {
          core::system::Live::latest!(41) + 1
        }
        "#;
    let compiled = compile(src);

    let core = runtime();
    core.deploy(
        &functions_from_module(&compiled),
        &named_hash(&compiled, "run"),
        |_| {},
    )
    .expect("deploys");

    let mut vm = core.build_vm();
    let err = vm
        .call(&named_hash(&compiled, "misuse"), Vec::new())
        .expect_err("a non-function argument must raise");
    let rendered = vm.runtime_error(err).to_string();
    assert!(
        rendered.contains("expected a function"),
        "the fault must name the contract: {rendered}"
    );
}
