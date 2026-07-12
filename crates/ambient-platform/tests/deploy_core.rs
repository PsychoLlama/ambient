//! Unit tests for the deploy core: additive object loading, the atomic
//! name-table swap, exact diff reporting, and pre-swap validation.
//!
//! Each test compiles real Ambient source and drives [`DeployRuntime`]
//! directly — no tasks involved; the task runtime is just one client
//! of this core.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{
    Binding, DeployError, DeployRuntime, Functions, Generation, functions_from_module,
};

/// Compile a pure test program against the core library, signatures
/// attached the way every build seam attaches them.
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
    merged.merge(&compiled);
    merged
}

fn runtime() -> DeployRuntime {
    DeployRuntime::new(Arc::new(Vm::new))
}

fn entry_hash(compiled: &CompiledModule) -> blake3::Hash {
    *compiled
        .function_names
        .get("run")
        .expect("test program has a `run` entry")
}

fn named_hash(compiled: &CompiledModule, name: &str) -> blake3::Hash {
    *compiled
        .function_names
        .get(name)
        .unwrap_or_else(|| panic!("test program defines `{name}`"))
}

/// A generation with `compiled`'s objects but hand-picked name bindings —
/// lets one compile exercise many binding shapes.
fn with_bindings(compiled: &CompiledModule, bindings: &[(&str, blake3::Hash)]) -> Functions {
    let base = functions_from_module(compiled);
    let bindings = bindings
        .iter()
        .map(|(name, hash)| {
            let binding = Binding {
                hash: *hash,
                signature: None,
            };
            (Arc::from(*name), binding)
        })
        .collect();
    Arc::new(Generation {
        functions: base.functions.clone(),
        values: base.values.clone(),
        natives: base.natives.clone(),
        bindings,
        migrations: Vec::new(),
    })
}

/// Old hashes stay resident: code loaded by an earlier deploy is still
/// callable after later deploys that no longer ship it.
#[test]
fn loading_is_additive_across_deploys() {
    let v1 = compile("pub fn run(): Number { 41 }");
    let v2 = compile("pub fn run(): Number { 42 }");
    let (entry1, entry2) = (entry_hash(&v1), entry_hash(&v2));
    assert_ne!(entry1, entry2, "the two builds must differ");

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &entry1, |_| {})
        .expect("first deploy succeeds");
    core.deploy(&functions_from_module(&v2), &entry2, |_| {})
        .expect("second deploy succeeds");

    // Both generations' entries are loaded and runnable side by side.
    assert!(core.lookup_function(&entry1).is_some());
    assert!(core.lookup_function(&entry2).is_some());
    let mut vm = core.build_vm();
    let old = vm.call(&entry1, Vec::new()).expect("old code still runs");
    let new = vm.call(&entry2, Vec::new()).expect("new code runs");
    assert_eq!(format!("{old:?}"), "Number(41.0)");
    assert_eq!(format!("{new:?}"), "Number(42.0)");
}

/// The deploy report's name diff is exact, hash against hash. Hand-built
/// bindings carry no signature, and missing data is never a silent
/// rebinding: a changed hash without both signatures classifies as
/// retire-and-fresh.
#[test]
fn diff_reports_added_retired_and_unchanged() {
    let compiled = compile(
        "pub fn run(): Number { 0 }
         pub fn one(): Number { 1 }
         pub fn two(): Number { 2 }",
    );
    let entry = entry_hash(&compiled);
    let (one, two) = (named_hash(&compiled, "one"), named_hash(&compiled, "two"));

    let core = runtime();
    let first = core
        .deploy(
            &with_bindings(&compiled, &[("a", one), ("b", two)]),
            &entry,
            |_| {},
        )
        .expect("first deploy succeeds");
    assert_eq!(first.names.added, vec![Arc::from("a"), Arc::from("b")]);
    assert!(first.names.rebound.is_empty());
    assert!(first.names.retired.is_empty());
    assert_eq!(first.names.unchanged, 0);

    let second = core
        .deploy(
            &with_bindings(&compiled, &[("a", one), ("b", one), ("c", two)]),
            &entry,
            |_| {},
        )
        .expect("second deploy succeeds");
    assert_eq!(second.names.added, vec![Arc::from("c")]);
    assert!(
        second.names.rebound.is_empty(),
        "a signatureless change must never report as a rebinding"
    );
    assert_eq!(second.names.retired, vec![Arc::from("b")]);
    assert_eq!(second.names.unchanged, 1);

    // And `latest` does not follow the retired lineage: b's old hash
    // resolves to itself, not to b's fresh binding.
    assert_eq!(core.latest(&two), two);
}

/// A changed name whose canonical signature is identical is a rebinding:
/// every hash of its lineage keeps resolving forward through `latest`,
/// transitively across deploys.
#[test]
fn same_signature_rebinding_resolves_old_refs_forward() {
    let v1 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 1 }",
    );
    let v2 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 2 }",
    );
    let v3 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 3 }",
    );
    let (t1, t2, t3) = (
        named_hash(&v1, "target"),
        named_hash(&v2, "target"),
        named_hash(&v3, "target"),
    );

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &entry_hash(&v1), |_| {})
        .expect("v1 deploys");
    let report = core
        .deploy(&functions_from_module(&v2), &entry_hash(&v2), |_| {})
        .expect("v2 deploys");
    assert!(report.names.rebound.contains(&Arc::from("target")));
    assert!(report.names.retired.is_empty());

    // One read per ref: v1's compile-time ref resolves to the current
    // binding of the name it was deployed under.
    assert_eq!(core.latest(&t1), t2);
    assert_eq!(core.latest(&t2), t2);

    // Transitive: after v3, the whole lineage resolves to the head.
    core.deploy(&functions_from_module(&v3), &entry_hash(&v3), |_| {})
        .expect("v3 deploys");
    assert_eq!(core.latest(&t1), t3);
    assert_eq!(core.latest(&t2), t3);

    // A hash never deployed under a name resolves to itself.
    let anonymous = blake3::hash(b"no name");
    assert_eq!(core.latest(&anonymous), anonymous);
}

/// A name whose canonical signature changed is retire-and-fresh: the
/// deploy report says so, and `latest` keeps resolving old refs to
/// themselves — evolving a live boundary's signature takes a new name.
#[test]
fn signature_change_is_retire_and_fresh() {
    let v1 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 1 }",
    );
    let v2 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(tag: String): Number { 2 }",
    );
    let (t1, t2) = (named_hash(&v1, "target"), named_hash(&v2, "target"));

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &entry_hash(&v1), |_| {})
        .expect("v1 deploys");
    let report = core
        .deploy(&functions_from_module(&v2), &entry_hash(&v2), |_| {})
        .expect("v2 deploys");
    assert_eq!(report.names.retired, vec![Arc::from("target")]);
    assert!(!report.names.rebound.contains(&Arc::from("target")));

    // Old refs are pinned to themselves; the fresh binding resolves to
    // itself too (its own lineage starts here).
    assert_eq!(core.latest(&t1), t1);
    assert_eq!(core.latest(&t2), t2);
    // The table itself did move: the name resolves to the fresh hash.
    assert_eq!(core.resolve("target").expect("target resolves").hash, t2);
}

/// Content addressing lets one hash be deployed under several names (two
/// same-bodied functions hash identically). While their bindings agree
/// the shared hash resolves forward; once they diverge no single answer
/// exists, so the shared hash resolves to itself.
#[test]
fn diverged_aliases_resolve_to_themselves() {
    // `a` and `b` have identical bodies — one content hash, two names.
    let v1 = compile(
        "pub fn run(): Number { 0 }
         pub fn a(): Number { 1 }
         pub fn b(): Number { 1 }",
    );
    let v2 = compile(
        "pub fn run(): Number { 0 }
         pub fn a(): Number { 2 }
         pub fn b(): Number { 1 }",
    );
    let shared = named_hash(&v1, "a");
    assert_eq!(
        shared,
        named_hash(&v1, "b"),
        "identical bodies must share one content hash"
    );

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &entry_hash(&v1), |_| {})
        .expect("v1 deploys");
    // Both names still bind the shared hash: resolving forward is
    // unambiguous (it goes nowhere new yet).
    assert_eq!(core.latest(&shared), shared);

    core.deploy(&functions_from_module(&v2), &entry_hash(&v2), |_| {})
        .expect("v2 deploys");
    // `a` moved on, `b` still binds the shared hash: the names disagree,
    // so the shared hash resolves to itself rather than guessing.
    assert_eq!(core.latest(&shared), shared);
    // `a`'s new hash is unambiguous.
    let a2 = named_hash(&v2, "a");
    assert_eq!(core.latest(&a2), a2);
}

/// A real build's generation binds every named item (dispatch symbols
/// excluded) and carries the canonical signatures the checker rendered.
#[test]
fn generations_carry_name_bindings_with_signatures() {
    let compiled = compile(
        "pub fn run(): Number { double(21) }
         fn double(n: Number): Number { n * 2 }",
    );
    let generation = functions_from_module(&compiled);

    let run = generation.bindings.get("run").expect("run is bound");
    assert_eq!(run.hash, entry_hash(&compiled));
    assert_eq!(run.signature.as_deref(), Some("fn() -> number"));
    let double = generation.bindings.get("double").expect("double is bound");
    assert_eq!(double.signature.as_deref(), Some("fn(number) -> number"));

    // Content dispatch symbols (`<uuid>::method`) are not names.
    assert!(
        generation
            .bindings
            .keys()
            .all(|name| !ambient_platform::deploy::is_dispatch_symbol(name)),
        "bindings must contain no dispatch symbols"
    );
}

/// Readers can never observe a torn table: two names deployed together
/// always agree, across many concurrent deploys.
#[test]
fn name_table_swap_is_atomic() {
    let compiled = compile(
        "pub fn run(): Number { 0 }
         pub fn one(): Number { 1 }
         pub fn two(): Number { 2 }",
    );
    let entry = entry_hash(&compiled);
    let (one, two) = (named_hash(&compiled, "one"), named_hash(&compiled, "two"));

    let core = Arc::new(runtime());
    core.deploy(
        &with_bindings(&compiled, &[("p", one), ("q", one)]),
        &entry,
        |_| {},
    )
    .expect("initial deploy succeeds");

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let core = Arc::clone(&core);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    // One snapshot: p and q are always deployed with the
                    // same hash, so a mixed pair means a torn table.
                    let table = core.name_table();
                    let p = table.get("p").expect("p is bound").hash;
                    let q = table.get("q").expect("q is bound").hash;
                    assert_eq!(p, q, "torn name table observed");
                }
            })
        })
        .collect();

    for i in 0..200 {
        let hash = if i % 2 == 0 { two } else { one };
        core.deploy(
            &with_bindings(&compiled, &[("p", hash), ("q", hash)]),
            &entry,
            |_| {},
        )
        .expect("deploy succeeds");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for reader in readers {
        reader.join().expect("reader saw no torn table");
    }

    // After the last deploy the table resolves to the final binding.
    assert_eq!(core.resolve("p").expect("p resolves").hash, one);
}

/// Validation failures reject the deploy before the swap: the name table
/// keeps its previous bindings.
#[test]
fn rejected_deploy_leaves_the_name_table_untouched() {
    let compiled = compile("pub fn run(): Number { 0 }");
    let entry = entry_hash(&compiled);

    let core = runtime();
    core.deploy(&with_bindings(&compiled, &[("a", entry)]), &entry, |_| {})
        .expect("first deploy succeeds");

    let unknown = blake3::hash(b"never deployed");
    let err = core
        .deploy(
            &with_bindings(&compiled, &[("a", unknown), ("fresh", entry)]),
            &entry,
            |_| {},
        )
        .expect_err("binding an unloaded object is rejected");
    assert!(
        matches!(err, DeployError::Validation(_)),
        "expected a validation rejection, got {err:?}"
    );

    // Previous generation untouched: `a` still resolves to the old hash,
    // and the rejected generation's fresh name never entered the table.
    assert_eq!(core.resolve("a").expect("a resolves").hash, entry);
    assert!(core.resolve("fresh").is_none());
}

/// An effectful function whose return type no caller constrains is a
/// rebinding across a redeploy, not a retirement — even when one build
/// monomorphized the return (a caller in the module) and the other left it
/// unconstrained. The deploy signature is the function's declared interface,
/// so an unannotated return renders the same (`var…`) regardless of
/// incidental callers; the effect row (the real interface) stays. This is
/// the bug behind the REPL's live-upgraded task body: without it a task's
/// redefinition retired the name and the running task kept the old code.
#[test]
fn unconstrained_effectful_return_rebinds_across_caller_context() {
    // `pinned` carries a caller (`use_tick`) that monomorphizes `tick`'s
    // return to `unit`; `open` leaves it unconstrained and gives `tick` a
    // different body (a second perform) so the hash genuinely changes.
    let pinned = compile(
        "pub fn run(): Number { 0 }
         unique(11111111-1111-1111-1111-111111111111) ability Beep { fn boop(): () { } }
         fn tick() { Beep::boop!() }
         fn use_tick(): () with Beep { tick() }",
    );
    let open = compile(
        "pub fn run(): Number { 0 }
         unique(11111111-1111-1111-1111-111111111111) ability Beep { fn boop(): () { } }
         fn tick() { Beep::boop!(); Beep::boop!() }",
    );
    let (h_pinned, h_open) = (named_hash(&pinned, "tick"), named_hash(&open, "tick"));
    assert_ne!(h_pinned, h_open, "the two `tick` bodies must differ");
    assert_eq!(
        pinned.signatures.get("tick"),
        open.signatures.get("tick"),
        "same source must render the same deploy signature regardless of \
         caller context"
    );

    // Monomorphized-then-open: the running (pinned) task redefined open.
    let core = runtime();
    core.deploy(
        &functions_from_module(&pinned),
        &entry_hash(&pinned),
        |_| {},
    )
    .expect("pinned deploys");
    let report = core
        .deploy(&functions_from_module(&open), &entry_hash(&open), |_| {})
        .expect("open deploys");
    assert!(
        report.names.rebound.contains(&Arc::from("tick")),
        "tick must rebind, got retired={:?}",
        report.names.retired
    );
    assert!(!report.names.retired.contains(&Arc::from("tick")));
    assert_eq!(core.latest(&h_pinned), h_open, "old refs follow the rebind");

    // Open-then-monomorphized: the reverse order must rebind too.
    let core = runtime();
    core.deploy(&functions_from_module(&open), &entry_hash(&open), |_| {})
        .expect("open deploys");
    let report = core
        .deploy(
            &functions_from_module(&pinned),
            &entry_hash(&pinned),
            |_| {},
        )
        .expect("pinned deploys");
    assert!(
        report.names.rebound.contains(&Arc::from("tick")),
        "tick must rebind in the reverse order too, got retired={:?}",
        report.names.retired
    );
    assert_eq!(core.latest(&h_open), h_pinned);
}

/// A plan reports exactly the diff a subsequent `deploy` then produces,
/// and changes nothing: not the name table, not the generation count, not
/// `latest` resolution.
#[test]
fn plan_reports_the_would_be_diff_without_committing() {
    let v1 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 1 }",
    );
    let v2 = compile(
        "pub fn run(): Number { 0 }
         pub fn target(): Number { 2 }
         pub fn fresh(): Number { 3 }",
    );
    let (t1, t2) = (named_hash(&v1, "target"), named_hash(&v2, "target"));

    let core = runtime();
    core.deploy(&functions_from_module(&v1), &entry_hash(&v1), |_| {})
        .expect("v1 deploys");

    // Snapshot the pre-plan world.
    let before_table = core.name_table();
    let before_target_latest = core.latest(&t1);

    let plan = core.plan(&functions_from_module(&v2), &entry_hash(&v2));
    assert!(plan.problems.is_empty(), "a clean plan has no problems");
    assert!(plan.names.rebound.contains(&Arc::from("target")));
    assert_eq!(plan.names.added, vec![Arc::from("fresh")]);
    assert!(plan.names.retired.is_empty());

    // The plan committed nothing: the name table is byte-identical, `fresh`
    // never entered it, and `latest` still resolves the old binding.
    assert_eq!(*core.name_table(), *before_table);
    assert!(core.resolve("fresh").is_none());
    assert_eq!(core.resolve("target").expect("target bound").hash, t1);
    assert_eq!(core.latest(&t1), before_target_latest);

    // The subsequent real deploy produces exactly the diff the plan predicted.
    let report = core
        .deploy(&functions_from_module(&v2), &entry_hash(&v2), |_| {})
        .expect("v2 deploys");
    assert_eq!(report.names.rebound, plan.names.rebound);
    assert_eq!(report.names.added, plan.names.added);
    assert_eq!(report.names.retired, plan.names.retired);
    assert_eq!(report.names.unchanged, plan.names.unchanged);
    // Only now does the world move.
    assert_eq!(core.resolve("target").expect("target bound").hash, t2);
    assert_eq!(core.latest(&t1), t2);
}

/// A plan whose entry is not loaded reports the problem — as a successful
/// plan, not an error (planning's whole purpose is reporting what would
/// happen).
#[test]
fn plan_reports_validation_problems_instead_of_erroring() {
    let compiled = compile("pub fn run(): Number { 0 }");
    let entry = entry_hash(&compiled);

    let core = runtime();
    core.deploy(&with_bindings(&compiled, &[("a", entry)]), &entry, |_| {})
        .expect("first deploy succeeds");

    // Bind a name to an object that was never loaded: validation rejects it.
    let unknown = blake3::hash(b"never deployed");
    let plan = core.plan(&with_bindings(&compiled, &[("a", unknown)]), &entry);
    assert!(
        plan.problems.iter().any(|p| p.contains("not loaded")),
        "the plan must report the unloaded binding: {:?}",
        plan.problems
    );
    // Still a would-be diff, and still no commit: `a` keeps its old hash.
    assert_eq!(core.resolve("a").expect("a bound").hash, entry);
}

/// The same instability afflicts an unannotated *parameter*: a caller pins
/// it to a concrete type, no caller leaves it a variable. The declared
/// interface renders the parameter as a variable either way, so a redeploy
/// that flips the caller context rebinds rather than retires.
#[test]
fn unconstrained_parameter_rebinds_across_caller_context() {
    // `echo` has an unannotated parameter and an annotated `Number` return.
    // `pinned`'s caller `echo(7)` monomorphizes the parameter to `number`;
    // `open` has no caller and a different body (an extra binding) so the
    // hash changes.
    let pinned = compile(
        "pub fn run(): Number { echo(7) }
         fn echo(x): Number { x }",
    );
    let open = compile(
        "pub fn run(): Number { 0 }
         fn echo(x): Number { let _z = 1; x }",
    );
    let (h_pinned, h_open) = (named_hash(&pinned, "echo"), named_hash(&open, "echo"));
    assert_ne!(h_pinned, h_open, "the two `echo` bodies must differ");
    assert_eq!(
        pinned.signatures.get("echo"),
        open.signatures.get("echo"),
        "an unannotated parameter must render the same regardless of caller"
    );

    let core = runtime();
    core.deploy(
        &functions_from_module(&pinned),
        &entry_hash(&pinned),
        |_| {},
    )
    .expect("pinned deploys");
    let report = core
        .deploy(&functions_from_module(&open), &entry_hash(&open), |_| {})
        .expect("open deploys");
    assert!(
        report.names.rebound.contains(&Arc::from("echo")),
        "echo must rebind, got retired={:?}",
        report.names.retired
    );
    assert_eq!(core.latest(&h_pinned), h_open);
}
