//! Unit tests for the deploy core: additive object loading, the atomic
//! name-table swap, exact diff reporting, and pre-swap validation.
//!
//! Each test compiles real Ambient source and drives [`DeployRuntime`]
//! directly — no processes involved; the process runtime is just this
//! core's first client.

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

/// The deploy report's name diff is exact, hash against hash.
#[test]
fn diff_reports_added_rebound_and_unchanged() {
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
    assert_eq!(first.names.unchanged, 0);

    let second = core
        .deploy(
            &with_bindings(&compiled, &[("a", one), ("b", one), ("c", two)]),
            &entry,
            |_| {},
        )
        .expect("second deploy succeeds");
    assert_eq!(second.names.added, vec![Arc::from("c")]);
    assert_eq!(second.names.rebound, vec![Arc::from("b")]);
    assert_eq!(second.names.unchanged, 1);
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
