//! End-to-end tests for the `State` ability (see ref/live-upgrade.md,
//! "State cells").
//!
//! Each test compiles real Ambient source against core + `core::system`
//! and exercises cells through the deploy core: the cell table is owned
//! by the `DeployRuntime` and shared by every VM it builds, across
//! generations — which is exactly what these tests pin.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_ability::Value;
use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployRuntime, functions_from_module};

/// Compile a test program against core + compiled `core::system` — the
/// same world `ambient run` checks against (the `State` default
/// implementations link by hash, so their compiled functions must merge
/// in).
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

/// Type-check a test program against core + `core::system` and return
/// the rendered errors (empty when it checks) — for pinning check-time
/// rejections without building a runtime.
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
        .map(std::string::ToString::to_string)
        .collect()
}

/// A deploy runtime whose VMs carry the platform stubs; the runtime's own
/// `build_vm` overlays the real `State` natives (and `live_latest`).
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

fn deploy(core: &DeployRuntime, compiled: &CompiledModule) {
    core.deploy(
        &functions_from_module(compiled),
        &named_hash(compiled, "run"),
        |_| {},
    )
    .expect("test program deploys");
}

fn call_number(core: &DeployRuntime, compiled: &CompiledModule, name: &str) -> f64 {
    let mut vm = core.build_vm();
    match vm.call(&named_hash(compiled, name), Vec::new()) {
        Ok(Value::Number(n)) => n,
        other => panic!("`{name}` should return a number, got {other:?}"),
    }
}

const V1: &str = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init!("counter", () => 1)
    }
    pub fn read(): Number with core::system::State {
      core::system::State::get!("counter")
    }
    pub fn put(value: Number): () with core::system::State {
      core::system::State::set!("counter", value)
    }
    pub fn bump(): Number with core::system::State {
      core::system::State::update!("counter", (x: Number) => x + 1)
    }
    "#;

/// Adoption is the point: `init` on an existing cell is a no-op, so the
/// entry re-runs on every deploy without resetting live state. The same
/// generation deployed into a fresh runtime *does* run its `make` — the
/// control that proves adoption (not a stale constant) kept the value.
#[test]
fn init_adopts_the_cell_across_deploys() {
    let v2_src = V1.replace("() => 1", "() => 99");
    let v1 = compile(V1);
    let v2 = compile(&v2_src);

    let core = runtime();
    deploy(&core, &v1);
    assert_eq!(call_number(&core, &v1, "read"), 1.0);

    deploy(&core, &v2);
    assert_eq!(
        call_number(&core, &v2, "read"),
        1.0,
        "the redeploy must adopt the live cell, not re-run make"
    );

    let fresh = runtime();
    deploy(&fresh, &v2);
    assert_eq!(
        call_number(&fresh, &v2, "read"),
        99.0,
        "a fresh runtime has no cell to adopt, so make runs"
    );
}

/// Cells live in the runtime, not in any VM: a write through one VM is
/// visible to a read through another.
#[test]
fn cells_are_shared_across_vms() {
    let v1 = compile(V1);
    let core = runtime();
    deploy(&core, &v1);

    let mut writer = core.build_vm();
    writer
        .call(&named_hash(&v1, "put"), vec![Value::Number(41.0)])
        .expect("set succeeds");

    assert_eq!(
        call_number(&core, &v1, "read"),
        41.0,
        "a second VM must see the first VM's write"
    );
}

/// `update` returns the value it stored.
#[test]
fn update_returns_the_new_value() {
    let v1 = compile(V1);
    let core = runtime();
    deploy(&core, &v1);

    assert_eq!(call_number(&core, &v1, "bump"), 2.0);
    assert_eq!(call_number(&core, &v1, "read"), 2.0);
}

/// The atomicity contract: concurrent `update`s from different VMs (own
/// threads, like concurrent reductions) never lose increments.
#[test]
fn update_is_atomic_under_concurrent_reduction() {
    let v1 = compile(V1);
    let core = runtime();
    deploy(&core, &v1);

    const THREADS: usize = 4;
    const BUMPS: usize = 50;
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                let mut vm = core.build_vm();
                let bump = named_hash(&v1, "bump");
                for _ in 0..BUMPS {
                    vm.call(&bump, Vec::new()).expect("bump succeeds");
                }
            });
        }
    });

    assert_eq!(
        call_number(&core, &v1, "read"),
        1.0 + (THREADS * BUMPS) as f64,
        "every increment must land exactly once"
    );
}

/// Touching the same cell from inside its own `update` is a fault (the
/// cell's lock is held), not a deadlock.
#[test]
fn reentrant_update_on_the_same_cell_faults() {
    let src = format!(
        r#"{V1}
    pub fn reenter(x: Number): Number with core::system::State {{
      core::system::State::get!("counter")
    }}
    pub fn reentrant_update(): Number with core::system::State {{
      core::system::State::update!("counter", reenter)
    }}
    "#
    );
    let compiled = compile(&src);
    let core = runtime();
    deploy(&core, &compiled);

    let mut vm = core.build_vm();
    let err = vm
        .call(&named_hash(&compiled, "reentrant_update"), Vec::new())
        .expect_err("reentrant access must fault");
    let rendered = vm.runtime_error(err).to_string();
    assert!(
        rendered.contains("reentrant"),
        "the fault must name the contract: {rendered}"
    );
    assert_eq!(
        call_number(&core, &compiled, "read"),
        1.0,
        "a faulted update must leave the cell unchanged"
    );
}

/// `get`/`set` on a never-initialized cell fault loudly: `init` is the
/// only creator, so creation stays a declaration in the entry.
#[test]
fn get_and_set_on_missing_cells_fault() {
    let src = r#"
    pub fn run(): () { }
    pub fn read_missing(): Number with core::system::State {
      core::system::State::get!("nope")
    }
    pub fn set_missing(): () with core::system::State {
      core::system::State::set!("nope", 1)
    }
    "#;
    let compiled = compile(src);
    let core = runtime();
    deploy(&core, &compiled);

    for entry in ["read_missing", "set_missing"] {
        let mut vm = core.build_vm();
        let err = vm
            .call(&named_hash(&compiled, entry), Vec::new())
            .expect_err("a missing cell must fault");
        let rendered = vm.runtime_error(err).to_string();
        assert!(
            rendered.contains("no cell named `nope`"),
            "`{entry}` must name the missing cell: {rendered}"
        );
    }
}

/// An exception inside `update`'s `f` re-raises at the caller's perform
/// site and the cell keeps its previous value.
#[test]
fn exception_in_update_leaves_the_cell_unchanged() {
    let src = format!(
        r#"{V1}
    pub fn boom(x: Number): Number with Exception {{
      Exception::throw!("boom");
      0 - 1
    }}
    pub fn explode(): Number with core::system::State {{
      core::system::State::update!("counter", boom)
    }}
    "#
    );
    let compiled = compile(&src);
    let core = runtime();
    deploy(&core, &compiled);

    let mut vm = core.build_vm();
    let err = vm
        .call(&named_hash(&compiled, "explode"), Vec::new())
        .expect_err("the exception must surface");
    let rendered = vm.runtime_error(err).to_string();
    assert!(rendered.contains("boom"), "got {rendered}");
    assert_eq!(call_number(&core, &compiled, "read"), 1.0);
}

/// `update`'s argument must be a function — since the fingerprint pass,
/// rejected at *check time*: the perform site constrains `f` to a
/// function shape to learn the cell type, so passing `41` no longer
/// compiles (it used to fault at runtime).
#[test]
fn update_rejects_non_functions() {
    let src = r#"
    pub fn run(): () with core::system::State {
      core::system::State::init!("n", () => 1)
    }
    pub fn misuse(): Number with core::system::State {
      core::system::State::update!("n", 41)
    }
    "#;
    let errors = check_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("type mismatch")),
        "a non-function `f` must be a check error: {errors:?}"
    );
}

/// IO handles live in cells for free: a TCP listener bound by
/// generation 1's entry and stashed in a cell is served by generation 2
/// after a deploy — the runtime owns both the name and the resource, so
/// there is no handoff. This drives a real localhost connection through
/// the shared `NetworkState` handle table, exactly the production wiring.
#[test]
fn listener_bound_in_gen_one_is_served_after_a_deploy() {
    const GEN1: &str = r#"
    fn bind(): Number with core::system::Network, Exception {
      match core::system::Network::listen!(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(message) => {
          Exception::throw!(message);
          0 - 1
        }
      }
    }
    pub fn run(): () with core::system::State {
      core::system::State::init!("listener", bind)
    }
    "#;
    // Gen 2 re-declares the same cell (adopts) and adds the serving code:
    // accept on the cell's handle, echo one message back.
    let gen2_src = format!(
        r#"{GEN1}
    pub fn serve(): Number with core::system::State, core::system::Network {{
      let listener = core::system::State::get!("listener");
      let conn = core::system::Network::accept!(listener).unwrap_or(0 - 1);
      let msg = core::system::Network::receive!(conn).unwrap_or(Binary::from([]));
      core::system::Network::send!(conn, msg);
      core::system::Network::close!(conn);
      msg.length()
    }}
    "#
    );

    let v1 = compile(GEN1);
    let v2 = compile(&gen2_src);

    // Real tokio-backed network table, shared by every VM the runtime
    // builds — the same shape as the CLI host's wiring.
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(ambient_platform::NetworkState::new(tokio.handle().clone()));
    let core = {
        let network = Arc::clone(&network);
        DeployRuntime::new(Arc::new(move || {
            let mut vm = Vm::new();
            vm.register_natives(&ambient_platform::stub_natives());
            vm.register_natives(&ambient_platform::network_natives(Arc::clone(&network)));
            vm
        }))
    };

    // Gen 1 binds the listener into the cell; gen 2 adopts it.
    deploy(&core, &v1);
    deploy(&core, &v2);

    // The cell holds the handle into the shared network table; ask the
    // table for the OS-assigned port.
    let listener_id = match core.cells().get("listener") {
        Ok(Value::Number(n)) => n as u64,
        other => panic!("the cell should hold a listener handle, got {other:?}"),
    };
    let addr = network
        .listener_addr(listener_id)
        .expect("gen-1's listener is still alive in the shared table");
    let port: u16 = addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("listener address has a port");

    // Serve from gen-2 code on one thread; connect as a real client from
    // another, through the same shared handle table.
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let mut vm = core.build_vm();
            vm.call(&named_hash(&v2, "serve"), Vec::new())
        });

        let client = network.connect("127.0.0.1", port).expect("client connects");
        network.send(client, b"ping").expect("client sends");
        let reply = network.receive(client).expect("client hears the echo");
        assert_eq!(reply, b"ping");
        network.close(client).expect("client closes");

        let served = server.join().expect("server thread completes");
        assert_eq!(
            served,
            Ok(Value::Number(4.0)),
            "gen-2 must have served the 4-byte message on gen-1's listener"
        );
    });
}

/// Without a deploy runtime — plain stubs, the compile-only/sandbox world
/// — `State` raises the standard not-wired fault. Cells stay deniable.
#[test]
fn state_stubs_raise_not_wired_without_a_deploy_runtime() {
    let v1 = compile(V1);

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

    let err = vm
        .call(&named_hash(&v1, "read"), Vec::new())
        .expect_err("the stub must fault");
    let rendered = vm.runtime_error(err).to_string();
    assert!(rendered.contains("not wired"), "got {rendered}");
}
