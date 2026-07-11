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

fn deploy_report(
    core: &DeployRuntime,
    compiled: &CompiledModule,
) -> ambient_platform::deploy::DeployReport {
    core.deploy(
        &functions_from_module(compiled),
        &named_hash(compiled, "run"),
        |_| {},
    )
    .expect("test program deploys")
}

fn deploy(core: &DeployRuntime, compiled: &CompiledModule) -> u64 {
    deploy_report(core, compiled).generation
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

    // Deploy the edit. The cell still holds generation 1's closure —
    // a pin, but not a warning: the new code is reachable through the
    // entry, so nothing about the change is unreachable.
    let report = deploy_report(&core, &v2);
    assert_eq!(report.generation, 2);
    assert!(
        report.warnings.is_empty(),
        "a benign rebind must not warn: {:?}",
        report.warnings
    );
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
    assert_eq!(
        pin.describe(),
        "`helper`",
        "the first pin is the most directly named hash"
    );
    assert!(
        pinned
            .pins
            .iter()
            .any(|pin| pin.describe() == "closure of `make_held`"),
        "the anonymous lambda is labeled by its named ancestor: {:?}",
        pinned.pins
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

/// The severed flavor of the unreachable-change warning: a name whose
/// signature changed retires (never rebinds), so a live old copy's own
/// late binding resolves to itself forever — the change can only land
/// on restart, and the deploy says so, naming the holder.
#[test]
fn a_signature_change_with_a_live_copy_warns_as_severed() {
    const V1: &str = r#"
    pub fn helper(x: Number): Number { x + 1 }
    fn make_held(): (Number) -> Number { helper }
    pub fn run(): () with core::system::State {
      core::system::State::init!("held", make_held)
    }
    "#;
    const V2: &str = r#"
    pub fn helper(x: Number, y: Number): Number { x + y }
    fn make_held(): (Number, Number) -> Number { helper }
    pub fn run(): () with core::system::State {
      core::system::State::init!("held", make_held)
    }
    "#;

    let core = runtime();
    let report = deploy_report(&core, &compile(V1));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    let report = deploy_report(&core, &compile(V2));
    let warning = report
        .warnings
        .iter()
        .find_map(|warning| match warning {
            ambient_platform::retire::DeployWarning::UnreachableChange {
                name,
                pinned_by,
                severed,
            } if name.contains("helper") => Some((pinned_by.clone(), *severed)),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected a severed warning for helper: {:?}",
                report.warnings
            )
        });
    assert_eq!(warning.0, RootOrigin::Cell(Arc::from("held")));
    assert!(warning.1, "a retired lineage is the severed flavor");
}

/// The orphaned flavor: the change rebinds fine, but no live re-entry
/// point (entry, task resolution) reaches the new code — only the
/// pinned old copy exists, so the change lands nowhere.
#[test]
fn a_change_no_reentry_point_reaches_warns_as_orphaned() {
    const V2: &str = r#"
    pub fn helper(x: Number): Number { x + 9 }
    fn make_held(): (Number) -> Number {
      (x: Number) => helper(x)
    }
    pub fn run(): () with core::system::State {
      core::system::State::init!("other", () => 0)
    }
    "#;

    let core = runtime();
    deploy(&core, &compile(HELD));
    let report = deploy_report(&core, &compile(V2));
    let warning = report
        .warnings
        .iter()
        .find_map(|warning| match warning {
            ambient_platform::retire::DeployWarning::UnreachableChange {
                name,
                pinned_by,
                severed,
            } if name.contains("helper") => Some((pinned_by.clone(), *severed)),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected an orphaned warning for helper: {:?}",
                report.warnings
            )
        });
    assert_eq!(warning.0, RootOrigin::Cell(Arc::from("held")));
    assert!(
        !warning.1,
        "a live lineage that nothing re-enters is the orphaned flavor"
    );
}

/// The ability-evolution drift channel: old live code performs a method
/// key that re-keyed under it (the default implementation changed), so
/// the new generation's handler cannot catch it — the perform falls
/// through to its old default, soundly but silently, and the deploy
/// warns.
#[test]
fn an_uncovered_method_key_performed_by_old_code_warns() {
    const V1: &str = r#"
    unique(A1B2C3D4-0000-0000-0000-00000000CC01) ability Ping {
      fn ping(): Number { 1 }
    }
    fn probe(): Number with Ping { Ping::ping!() }
    pub fn run(): () with core::system::State {
      core::system::State::init!("held", () => probe)
    }
    "#;
    const V2: &str = r#"
    unique(A1B2C3D4-0000-0000-0000-00000000CC01) ability Ping {
      fn ping(): Number { 2 }
    }
    fn probe(): Number with Ping { Ping::ping!() }
    pub fn run(): Number with core::system::State {
      core::system::State::init!("held", () => probe);
      with { Ping::ping() => resume(7) } handle probe()
    }
    "#;

    let core = runtime();
    let report = deploy_report(&core, &compile(V1));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    let report = deploy_report(&core, &compile(V2));
    assert!(
        report.warnings.iter().any(|warning| matches!(
            warning,
            ambient_platform::retire::DeployWarning::UncoveredMethodKey { .. }
        )),
        "the re-keyed method performed by the pinned closure should warn: {:?}",
        report.warnings
    );
}

/// The `store gc` integration and its safety contract: purging with the
/// trace's reachable set as extra roots never removes an object the
/// running system can still reach (a pinned old generation's code),
/// while a *retired* generation's objects — nothing live, nothing
/// names-rooted — are exactly what gets purged.
#[test]
fn disk_gc_keeps_pinned_objects_and_purges_retired_ones() {
    let v2_src = HELD.replace("x + 1", "x + 2");
    let v1 = compile(HELD);
    let v2 = compile(&v2_src);
    let helper_v1 = named_hash(&v1, "helper");
    let helper_v2 = named_hash(&v2, "helper");

    let dir = tempfile::tempdir().expect("temp dir");
    let disk =
        ambient_engine::disk_store::DiskStore::open(dir.path().join("store")).expect("open store");

    let core = runtime();
    deploy(&core, &v1);
    disk.put_module(&v1).expect("persist v1");
    deploy(&core, &v2);
    disk.put_module(&v2).expect("persist v2");

    // Generation 1 is pinned by the cell-held closure: gc with the
    // trace's reachable set must keep its code even though the names
    // index has moved on to generation 2.
    let report = core.retirement();
    assert!(report.retired.is_empty());
    disk.gc(&report.reachable).expect("gc");
    assert!(
        disk.contains(&helper_v1),
        "gc must never purge a hash the running system reaches"
    );
    assert!(disk.contains(&helper_v2));

    // Drop the pin: generation 1 retires, and its unique objects are
    // now exactly the garbage.
    call(&core, &v2, "drop_held");
    let report = core.retirement();
    assert_eq!(report.newly_retired, vec![1]);
    disk.gc(&report.reachable).expect("gc");
    assert!(
        !disk.contains(&helper_v1),
        "a retired generation's unique objects are purgeable"
    );
    assert!(disk.contains(&helper_v2));
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
