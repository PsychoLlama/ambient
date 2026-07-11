//! Retirement-trace tests (see `ref/live-upgrade.md`, "Retirement").
//!
//! Each test compiles real Ambient source, deploys generations through a
//! `DeployRuntime`, and asks the runtime's retirement query which
//! generations are still live and why. The trace's contract: a value
//! held by the runtime (here, a `State` cell) pins the generation whose
//! code it references; dropping the value retires the generation —
//! permanently.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployRuntime, functions_from_module};
use ambient_platform::retire::RootOrigin;

/// Compile a test program against core + compiled `core::system` — the
/// same world `ambient run` checks against.
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

fn deploy(core: &DeployRuntime, compiled: &CompiledModule) -> u64 {
    core.deploy(
        &functions_from_module(compiled),
        &named_hash(compiled, "run"),
        |_| {},
    )
    .expect("test program deploys")
    .generation
}

fn call(core: &DeployRuntime, compiled: &CompiledModule, name: &str) {
    let mut vm = core.build_vm();
    vm.call(&named_hash(compiled, name), Vec::new())
        .unwrap_or_else(|e| panic!("`{name}` runs: {e:?}"));
}

/// The entry parks a closure over `helper` in a cell. Editing `helper`
/// re-keys the closure's lambda in the next build, so the cell keeps
/// generation 1's lambda alive — until `drop_held` overwrites it.
const HELD: &str = r#"
    pub fn helper(x: Number): Number { x + 1 }
    fn make_held(): (Number) -> Number {
      (x: Number) => helper(x)
    }
    pub fn run(): () with core::system::State {
      core::system::State::init!("held", make_held)
    }
    pub fn drop_held(): () with core::system::State {
      core::system::State::set!("held", 0)
    }
    "#;

/// The trace's core contract, end to end: a closure in a cell pins the
/// generation whose code it references (diagnosed with the holding cell
/// and the closure's named ancestor); overwriting the cell retires the
/// generation — permanently.
#[test]
fn a_closure_in_a_cell_pins_its_generation_until_dropped() {
    let v2_src = HELD.replace("x + 1", "x + 2");
    let v1 = compile(HELD);
    let v2 = compile(&v2_src);

    let core = runtime();
    assert_eq!(deploy(&core, &v1), 1);

    // One generation deployed: nothing is old, nothing to retire.
    let report = core.retirement();
    assert_eq!(report.current, Some(1));
    assert!(report.retired.is_empty());
    assert!(report.pinned.is_empty());

    // Deploy the edit. The cell still holds generation 1's closure.
    assert_eq!(deploy(&core, &v2), 2);
    let report = core.retirement();
    assert_eq!(report.current, Some(2));
    assert!(
        report.retired.is_empty(),
        "the held closure must keep generation 1 live"
    );
    let pinned = report
        .pinned
        .iter()
        .find(|generation| generation.id == 1)
        .expect("generation 1 is pinned");
    let pin = pinned
        .pins
        .first()
        .expect("the pin names what holds generation 1");
    assert_eq!(
        pin.root,
        RootOrigin::Cell(Arc::from("held")),
        "provenance should name the holding cell"
    );
    assert!(
        pin.describe().contains("closure of `make_held`"),
        "the anonymous lambda should be labeled by its named ancestor, got {}",
        pin.describe()
    );
    assert!(
        report.reachable.contains(&pin.hash),
        "pinned hashes are reachable (gc must keep them)"
    );

    // Drop the closure: the only reference to generation 1's code goes
    // away, and the generation retires.
    call(&core, &v2, "drop_held");
    let report = core.retirement();
    assert_eq!(report.newly_retired, vec![1]);
    assert_eq!(report.retired, vec![1]);

    // Retirement is sticky and reported as a transition exactly once.
    let report = core.retirement();
    assert!(report.newly_retired.is_empty());
    assert_eq!(report.retired, vec![1]);
}

/// Attribution is by *latest* shipper: redeploying identical code
/// re-ships every hash, so the previous generation owns nothing unique
/// and retires even though a cell still holds "its" closure — the
/// closure's code is byte-identical in the new generation.
#[test]
fn an_identical_redeploy_retires_the_previous_generation() {
    let v1 = compile(HELD);
    let core = runtime();
    assert_eq!(deploy(&core, &v1), 1);
    assert_eq!(deploy(&core, &v1), 2);

    let report = core.retirement();
    assert_eq!(report.current, Some(2));
    assert_eq!(
        report.newly_retired,
        vec![1],
        "identical code means nothing is uniquely generation 1's"
    );
}

/// A name that is still bound is reachable through late-bound
/// resolution even when no runtime value holds it: the current name
/// table is itself a root, so the current generation is never reported
/// retired or pinned.
#[test]
fn the_current_generation_is_never_retired() {
    let v1 = compile(HELD);
    let core = runtime();
    deploy(&core, &v1);

    for _ in 0..2 {
        let report = core.retirement();
        assert_eq!(report.current, Some(1));
        assert!(report.retired.is_empty());
        assert!(report.pinned.is_empty());
    }
}
