//! End-to-end tests for the migration contract (see ref/live-upgrade.md,
//! "Migration").
//!
//! `State::init_versioned<Old, New>(name, make, migrate)` dispatches on
//! the cell's recorded **fingerprint** — the canonical rendering of the
//! static type the cell was last written at, threaded through every write
//! perform by the compiler:
//!
//! | Cell state          | Action                                      |
//! |---------------------|---------------------------------------------|
//! | no cell             | `make()`                                    |
//! | fingerprint = `New` | adopt (the every-deploy no-op)              |
//! | fingerprint = `Old` | `migrate(old)`; fingerprint advances        |
//! | anything else       | **deploy rejected** at validation, pre-swap |
//!
//! The reject arm is statically checkable only for literal cell names
//! (the compiler records those as `MigrationRecord`s); a computed name
//! falls back to a perform-time fault, pinned here too.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_ability::Value;
use ambient_engine::compiler::{
    CompileOptions, CompiledModule, MigrationRecord, compile_module_with_options,
};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployError, DeployReport, DeployRuntime, functions_from_module};

/// Compile a test program against core + compiled `core::system` (the
/// `state_cells.rs` harness; signatures copied so rebinding works).
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

fn deploy(core: &DeployRuntime, compiled: &CompiledModule) -> Result<DeployReport, DeployError> {
    core.deploy(
        &functions_from_module(compiled),
        &named_hash(compiled, "run"),
        |_| {},
    )
}

fn call_number(core: &DeployRuntime, compiled: &CompiledModule, name: &str) -> f64 {
    let mut vm = core.build_vm();
    match vm.call(&named_hash(compiled, name), Vec::new()) {
        Ok(Value::Number(n)) => n,
        other => panic!("`{name}` should return a number, got {other:?}"),
    }
}

fn fingerprint(core: &DeployRuntime, cell: &str) -> String {
    core.cells()
        .fingerprint(cell)
        .expect("cell table is healthy")
        .unwrap_or_else(|| panic!("cell `{cell}` exists"))
        .to_string()
}

/// Generation 1: the cell holds a plain `Number`.
const GEN1: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init!("stats", () => 41)
    }
    pub fn read(): Number with core::system::State {
      core::system::State::get!("stats")
    }
    "#;

/// Generation 2: the cell's type evolved to a record; the old `Number`
/// shape survives in source only as the migration's parameter type.
const GEN2: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!(
        "stats",
        () => ({ count: 0 }),
        (old: Number) => ({ count: old + 1 })
      )
    }
    fn count_of(stats: { count: Number }): Number {
      stats.count
    }
    pub fn count(): Number with core::system::State {
      count_of(core::system::State::get!("stats"))
    }
    "#;

/// The three non-reject arms in sequence: `make` on a fresh runtime,
/// `migrate` when the fingerprint matches the old type, adopt on
/// redeploy once it matches the new.
#[test]
fn init_versioned_dispatches_make_migrate_and_adopt() {
    let v1 = compile(GEN1);
    let v2 = compile(GEN2);

    // No cell → make.
    let fresh = runtime();
    deploy(&fresh, &v2).expect("make arm deploys");
    assert_eq!(
        call_number(&fresh, &v2, "count"),
        0.0,
        "with no cell, make must create the record"
    );

    // Fingerprint Old → migrate, and the fingerprint advances.
    let core = runtime();
    deploy(&core, &v1).expect("gen 1 deploys");
    assert_eq!(call_number(&core, &v1, "read"), 41.0);
    assert_eq!(fingerprint(&core, "stats"), "number");

    deploy(&core, &v2).expect("the migration deploys");
    assert_eq!(
        call_number(&core, &v2, "count"),
        42.0,
        "migrate must transform the live Number, not re-run make"
    );
    assert_eq!(fingerprint(&core, "stats"), "{count: number}");

    // Fingerprint New → adopt: neither make nor migrate runs again.
    deploy(&core, &v2).expect("the redeploy adopts");
    assert_eq!(
        call_number(&core, &v2, "count"),
        42.0,
        "the redeploy must adopt the migrated cell untouched"
    );
}

/// Migrations compose across deploys: each hop's `New` is the next hop's
/// `Old`, so a lineage of relic types walks forward one deploy at a time.
#[test]
fn migrations_chain_across_generations() {
    const GEN3: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!(
        "stats",
        () => ({ count: 0, total: 0 }),
        (old: { count: Number }) => ({ count: old.count, total: 100 })
      )
    }
    fn total_of(stats: { count: Number, total: Number }): Number {
      stats.total + stats.count
    }
    pub fn total(): Number with core::system::State {
      total_of(core::system::State::get!("stats"))
    }
    "#;
    let core = runtime();
    deploy(&core, &compile(GEN1)).expect("gen 1 deploys");
    deploy(&core, &compile(GEN2)).expect("gen 2 migrates");
    let v3 = compile(GEN3);
    deploy(&core, &v3).expect("gen 3 migrates again");

    assert_eq!(
        call_number(&core, &v3, "total"),
        142.0,
        "both migrations must have applied, in order"
    );
    assert_eq!(
        fingerprint(&core, "stats"),
        "{count: number, total: number}"
    );
}

/// The reject arm: a statically-named migration whose cell matches
/// neither type fails validation **pre-swap** — the name table and every
/// cell stay exactly as they were, so the running system is untouched.
#[test]
fn mismatched_migration_rejects_the_deploy_pre_swap() {
    // Expects the cell to be a Bool (it is a Number): neither old nor new.
    const MISMATCH: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!(
        "stats",
        () => "fresh",
        (old: Bool) => "migrated"
      )
    }
    pub fn probe(): Number { 42 }
    "#;
    let v1 = compile(GEN1);
    let v2 = compile(MISMATCH);

    let core = runtime();
    deploy(&core, &v1).expect("gen 1 deploys");
    let run_v1 = core.resolve("run").expect("gen 1 bound `run`").hash;

    let err = deploy(&core, &v2).expect_err("the mismatch must reject the deploy");
    let rendered = err.to_string();
    assert!(
        matches!(err, DeployError::Validation(_)),
        "must reject at validation, not fault in the entry: {err:?}"
    );
    assert!(
        rendered.contains("`stats`")
            && rendered.contains("number")
            && rendered.contains("bool")
            && rendered.contains("string"),
        "the rejection must name the cell and all three types: {rendered}"
    );

    // Pre-swap means untouched: the cell, its fingerprint, and the name
    // table are all still generation 1's.
    assert_eq!(call_number(&core, &v1, "read"), 41.0);
    assert_eq!(fingerprint(&core, "stats"), "number");
    assert_eq!(
        core.resolve("run").expect("`run` is still bound").hash,
        run_v1,
        "the rejected generation must not rebind names"
    );
    assert!(
        core.resolve("probe").is_none(),
        "the rejected generation must not add names"
    );
}

/// A dry-run `plan` runs the same pre-swap migration validation as
/// `deploy`, but reports the mismatch as a *successful* plan (problems in
/// the report) rather than an error — and touches nothing: the live cell,
/// its fingerprint, the name table, and the generation count all stay
/// exactly as generation 1 left them.
#[test]
fn plan_reports_migration_mismatch_without_touching_the_system() {
    const MISMATCH: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!(
        "stats",
        () => "fresh",
        (old: Bool) => "migrated"
      )
    }
    pub fn probe(): Number { 42 }
    "#;
    let v1 = compile(GEN1);
    let v2 = compile(MISMATCH);

    let core = runtime();
    deploy(&core, &v1).expect("gen 1 deploys");
    let run_v1 = core.resolve("run").expect("gen 1 bound `run`").hash;

    let plan = core.plan(&functions_from_module(&v2), &named_hash(&v2, "run"));
    assert!(
        plan.problems
            .iter()
            .any(|p| { p.contains("`stats`") && p.contains("number") && p.contains("bool") }),
        "the plan must report the cell and the mismatching types: {:?}",
        plan.problems
    );

    // Nothing committed: the cell still reads 41 at fingerprint `number`,
    // `run` still binds generation 1's hash, and the rejected generation's
    // fresh `probe` never entered the name table.
    assert_eq!(call_number(&core, &v1, "read"), 41.0);
    assert_eq!(fingerprint(&core, "stats"), "number");
    assert_eq!(core.resolve("run").expect("`run` still bound").hash, run_v1);
    assert!(
        core.resolve("probe").is_none(),
        "a plan must not add the candidate generation's names"
    );

    // And a real deploy of the same generation still rejects it, proving
    // the plan left the reject arm's precondition intact.
    let err = deploy(&core, &v2).expect_err("the mismatch still rejects the real deploy");
    assert!(matches!(err, DeployError::Validation(_)), "{err:?}");
}

/// A computed cell name cannot be validated pre-swap (there is no static
/// record), so the same mismatch faults at the perform instead — during
/// reconciliation, surfacing as an entry error.
#[test]
fn computed_cell_names_fault_at_perform_time() {
    const DYNAMIC: &str = r#"
    fn ensure(name: String): () with core::system::State {
      core::system::State::init_versioned!(
        name,
        () => "fresh",
        (old: Bool) => "migrated"
      )
    }
    pub fn run(): () with core::system::State {
      ensure("stats")
    }
    "#;
    let v1 = compile(GEN1);
    let v2 = compile(DYNAMIC);
    assert!(
        functions_from_module(&v2).migrations.is_empty(),
        "a computed name must not produce a static migration record"
    );

    let core = runtime();
    deploy(&core, &v1).expect("gen 1 deploys");
    let err = deploy(&core, &v2).expect_err("the mismatch must fault");
    assert!(
        matches!(err, DeployError::Entry(_)),
        "a computed name defers to a perform-time fault: {err:?}"
    );
    assert!(
        err.to_string().contains("neither"),
        "the fault must explain the fingerprint mismatch: {err}"
    );
    assert_eq!(
        call_number(&core, &v1, "read"),
        41.0,
        "a faulted migrate must leave the cell unchanged"
    );
}

/// An exception inside `migrate` re-raises at the perform site (the
/// entry), and the cell keeps its previous value *and* fingerprint, so a
/// fixed migration can be redeployed.
#[test]
fn faulting_migrate_leaves_value_and_fingerprint() {
    const EXPLODING: &str = r#"
    fn boom(old: Number): String with Exception {
      Exception::throw!("migration exploded");
      "unreachable"
    }
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!("stats", () => "fresh", boom)
    }
    "#;
    let v1 = compile(GEN1);
    let v2 = compile(EXPLODING);

    let core = runtime();
    deploy(&core, &v1).expect("gen 1 deploys");
    let err = deploy(&core, &v2).expect_err("the exploding migrate must fault the entry");
    assert!(err.to_string().contains("migration exploded"), "{err}");
    assert_eq!(call_number(&core, &v1, "read"), 41.0);
    assert_eq!(
        fingerprint(&core, "stats"),
        "number",
        "a faulted migrate must not advance the fingerprint"
    );
}

/// Every write path stamps the writer's fingerprint: `set` moves it, so
/// the next migration dispatches on what was actually written last.
#[test]
fn set_restamps_the_fingerprint() {
    let src = format!(
        r#"{GEN1}
    pub fn overwrite(): () with core::system::State {{
      core::system::State::set!("stats", "now a string")
    }}
    "#
    );
    let compiled = compile(&src);
    let core = runtime();
    deploy(&core, &compiled).expect("deploys");
    assert_eq!(fingerprint(&core, "stats"), "number");

    let mut vm = core.build_vm();
    vm.call(&named_hash(&compiled, "overwrite"), Vec::new())
        .expect("set succeeds");
    assert_eq!(
        fingerprint(&core, "stats"),
        "string",
        "set must stamp the type it wrote at"
    );
}

/// Fingerprint stability: the same source renders the same fingerprints
/// on every compile — the property deploy validation and the cell table
/// compare by. Pins the exact canonical strings.
#[test]
fn fingerprints_are_byte_stable_across_compiles() {
    let a = compile(GEN2);
    let b = compile(GEN2);
    assert_eq!(
        a.migrations, b.migrations,
        "two compiles must render identical fingerprints"
    );
    assert_eq!(
        a.migrations,
        vec![MigrationRecord {
            cell: "stats".into(),
            old: "number".into(),
            new: "{count: number}".into(),
        }],
        "the canonical renderings are pinned bytes"
    );
}

/// Run the parse/check pipeline against core + `core::system` and return
/// the checker's diagnostics as strings.
fn check_errors(src: &str) -> Vec<String> {
    let module = ambient_parser::parse(src).expect("test program parses");
    let mut registry = ModuleRegistry::new();
    let mut module_function_hashes = HashMap::new();
    ambient_engine::build::compile_core_modules(&mut registry, &mut module_function_hashes, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules compile");
    registry
        .natives_mut()
        .merge(&ambient_platform::stub_natives());
    ambient_engine::build::compile_declaration_modules(
        &mut registry,
        &mut module_function_hashes,
        ambient_platform::platform_modules(),
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("core::system compiles");
    let path = ModulePath::root();
    registry.register(&path, Arc::new(module.clone()));
    check_module_with_registry(module, &path, &registry)
        .errors
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// A cell write at a type mentioning the enclosing function's type
/// parameter cannot be fingerprinted (it would mean a different type per
/// instantiation) and is rejected at check time.
#[test]
fn generic_cell_writes_are_check_errors() {
    let src = r#"
    pub fn stash<T>(value: T): () with core::system::State {
      core::system::State::set!("g", value)
    }
    "#;
    let errors = check_errors(src);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("cannot fingerprint a `State` cell at generic type")),
        "a generic write must be a check error: {errors:?}"
    );
}

/// `init_versioned` type-checks with **both** function params effectful
/// and at distinct old/new types — the shape that drove the parser fix
/// (two effectful function-typed params, each with a `with` clause,
/// followed by the compiler-supplied fingerprint params). `make`'s and
/// `migrate`'s effect rows stay local to the call, so the entry's own row
/// is `State` alone.
#[test]
fn init_versioned_accepts_effectful_make_and_migrate() {
    let src = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init_versioned!(
        "stats",
        () => {
          core::system::Stdio::out!("make");
          ({ count: 0 })
        },
        (old: Number) => {
          core::system::Stdio::out!("migrate");
          ({ count: old + 1 })
        }
      )
    }
    "#;
    let errors = check_errors(src);
    assert!(
        errors.is_empty(),
        "effectful make and migrate at distinct types must type-check: \
         {errors:?}"
    );
}
