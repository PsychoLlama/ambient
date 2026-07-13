//! End-to-end tests for drain and interruptible performs (see
//! ref/live-upgrade.md, "Drain").
//!
//! Each test compiles real Ambient source against core + `core::system`,
//! runs it on a VM wired with the interruptible natives
//! (`install_drain_natives`), and drives a [`DrainSignal`] from the test
//! thread — the raw runtime hook Phase 5 exercises drain against (the
//! `Task` ability arrives in Phase 6). The contract under test:
//!
//! - The unwind lands **only at interruptible performs**: straight-line
//!   code before the interrupted call always runs to completion.
//! - The nearest `Drain::requested` arm runs cleanup and yields the
//!   computation's value; with no arm in scope the delivery is a fault.
//! - A drain deadline hard-stops a computation that never reaches an
//!   interruptible perform.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ambient_ability::{Value, VmError};
use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployRuntime, functions_from_module};
use ambient_platform::{DrainSignal, TcpState, install_drain_natives};

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

/// A deploy runtime whose VMs carry the platform stubs plus real network
/// natives against `network` — the production wiring shape. Drain wiring
/// is per computation, added by each test with [`install_drain_natives`].
fn runtime(network: &Arc<TcpState>) -> DeployRuntime {
    let network = Arc::clone(network);
    DeployRuntime::new(Arc::new(move || {
        let mut vm = Vm::new();
        vm.register_natives(&ambient_platform::stub_natives());
        vm.register_natives(&ambient_platform::tcp_natives(Arc::clone(&network)));
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

/// A drain-wired VM for one computation.
fn drainable_vm(core: &DeployRuntime, network: &Arc<TcpState>, signal: &Arc<DrainSignal>) -> Vm {
    let mut vm = core.build_vm();
    install_drain_natives(&mut vm, network, signal);
    vm
}

/// Poll the runtime's cell table until `cell` holds `expected` (the
/// computation has reached that checkpoint) or a generous timeout trips.
fn await_checkpoint(core: &DeployRuntime, cell: &str, expected: f64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Value::Number(n)) = core.cells().get(cell)
            && (n - expected).abs() < f64::EPSILON
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for cell `{cell}` to reach {expected}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A computation that checkpoints into a cell, blocks in an
/// interruptible `Time::wait`, and would checkpoint again if it ever
/// resumed. The `Drain::requested` arm yields 42.
const WAIT_LOOP: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State {
      core::system::State::init!("checkpoint", () => 0)
    }

    fn serve(): Number with core::system::Time, core::system::State {
      core::system::State::set!("checkpoint", 1);
      core::system::Time::wait!(Duration::from_secs(3600));
      core::system::State::set!("checkpoint", 2);
      -1
    }

    pub fn main(): Number with core::system::Time, core::system::State {
      with { core::system::Drain::requested() => 42 } handle serve()
    }
    "#;

/// A drain request interrupts a computation blocked in `Time::wait`;
/// the `Drain::requested` arm runs and its value is the computation's
/// value. The continuation is discarded: the write after the wait never
/// happens.
#[test]
fn drain_unwinds_a_blocked_wait_and_the_arm_yields_the_value() {
    let compiled = compile(WAIT_LOOP);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let signal = DrainSignal::new();
    let mut vm = drainable_vm(&core, &network, &signal);
    let main = named_hash(&compiled, "main");

    let computation = std::thread::spawn(move || vm.call(&main, Vec::new()));
    await_checkpoint(&core, "checkpoint", 1.0);
    signal.request();

    let result = computation.join().expect("computation thread completes");
    signal.mark_complete();
    assert_eq!(
        result,
        Ok(Value::Number(42.0)),
        "the Drain::requested arm's value must be the computation's value"
    );
    assert_eq!(
        core.cells().get("checkpoint").expect("cell exists"),
        Value::Number(1.0),
        "the unwind must discard the continuation: the post-wait write never runs"
    );
}

/// The unwind lands only at interruptible performs: with the drain
/// requested *before* the computation starts, the straight-line code up
/// to the first interruptible perform still runs to completion, and the
/// unwind is delivered exactly there.
#[test]
fn code_before_the_interruptible_perform_runs_to_completion() {
    let compiled = compile(WAIT_LOOP);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let signal = DrainSignal::new();
    signal.request();
    let mut vm = drainable_vm(&core, &network, &signal);

    let result = vm.call(&named_hash(&compiled, "main"), Vec::new());
    assert_eq!(result, Ok(Value::Number(42.0)));
    assert_eq!(
        core.cells().get("checkpoint").expect("cell exists"),
        Value::Number(1.0),
        "the checkpoint before the wait must have been written"
    );
}

/// A computation blocked in `Tcp::accept` (a listener bound into a
/// cell, the Phase 3 pattern) is interrupted at the accept, and cleanup
/// closes over the loop's state.
#[test]
fn drain_unwinds_a_blocked_accept() {
    const SRC: &str = r#"
    fn bind(): Number with core::system::Tcp, Exception {
      match core::system::Tcp::listen!(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(message) => {
          Exception::throw!(message);
          -1
        }
      }
    }
    pub fn run(): () with core::system::State {
      core::system::State::init!("listener", bind)
    }
    fn serve(): Number with core::system::State, core::system::Tcp {
      let listener = core::system::State::get!("listener");
      let conn = core::system::Tcp::accept!(listener).unwrap_or(-1);
      conn
    }
    pub fn main(): Number with core::system::State, core::system::Tcp {
      with { core::system::Drain::requested() => 7 } handle serve()
    }
    "#;

    let compiled = compile(SRC);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let signal = DrainSignal::new();
    let mut vm = drainable_vm(&core, &network, &signal);
    let main = named_hash(&compiled, "main");

    let computation = std::thread::spawn(move || vm.call(&main, Vec::new()));
    // No checkpoint can prove "blocked in accept"; the signal is
    // race-free by construction (flag checked before blocking, waiter
    // registered before the flag re-check), so a small delay only makes
    // the interesting interleaving — a genuinely blocked accept — the
    // overwhelmingly common one.
    std::thread::sleep(Duration::from_millis(30));
    signal.request();

    let result = computation.join().expect("computation thread completes");
    signal.mark_complete();
    assert_eq!(result, Ok(Value::Number(7.0)));
}

/// A computation blocked in `Tcp::receive` mid-conversation: the
/// accept completes normally (a real client connects), the receive is
/// the interruption point.
#[test]
fn drain_unwinds_a_blocked_receive() {
    const SRC: &str = r#"
    fn bind(): Number with core::system::Tcp, Exception {
      match core::system::Tcp::listen!(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(message) => {
          Exception::throw!(message);
          -1
        }
      }
    }
    pub fn run(): () with core::system::State {
      core::system::State::init!("listener", bind);
      core::system::State::init!("conn", () => -1)
    }
    fn serve(): Number with core::system::State, core::system::Tcp {
      let listener = core::system::State::get!("listener");
      let conn = core::system::Tcp::accept!(listener).unwrap_or(-1);
      core::system::State::set!("conn", conn);
      let msg = core::system::Tcp::receive!(conn).unwrap_or(Binary::from([]));
      msg.length()
    }
    pub fn main(): Number with core::system::State, core::system::Tcp {
      with { core::system::Drain::requested() => 8 } handle serve()
    }
    "#;

    let compiled = compile(SRC);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let listener_id = match core.cells().get("listener") {
        Ok(Value::Number(n)) => n as u64,
        other => panic!("the cell should hold a listener handle, got {other:?}"),
    };
    let addr = network.listener_addr(listener_id).expect("listener alive");
    let port: u16 = addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("listener address has a port");

    let signal = DrainSignal::new();
    let mut vm = drainable_vm(&core, &network, &signal);
    let main = named_hash(&compiled, "main");

    let computation = std::thread::spawn(move || vm.call(&main, Vec::new()));

    // Connect a real client (the accept completes normally), then wait
    // for the server to publish the connection — it is now headed into
    // the blocking receive — and drain it.
    let _client = network.connect("127.0.0.1", port).expect("client connects");
    await_checkpoint_not(&core, "conn", -1.0);
    signal.request();

    let result = computation.join().expect("computation thread completes");
    signal.mark_complete();
    assert_eq!(
        result,
        Ok(Value::Number(8.0)),
        "the receive must be the interruption point (the accept completed)"
    );
}

/// Poll until `cell` holds something other than `unexpected`.
fn await_checkpoint_not(core: &DeployRuntime, cell: &str, unexpected: f64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Value::Number(n)) = core.cells().get(cell)
            && (n - unexpected).abs() > f64::EPSILON
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for cell `{cell}` to leave {unexpected}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// With no `Drain::requested` arm in scope, the delivery is an
/// unhandled-ability fault the draining host observes — `requested` is
/// abstract, so there is no default implementation to fall back to.
#[test]
fn an_unhandled_drain_is_a_fault() {
    const SRC: &str = r#"
    use core::time::Duration;

    pub fn run(): () {
      ()
    }

    pub fn main(): Number with core::system::Time {
      core::system::Time::wait!(Duration::from_secs(3600));
      1
    }
    "#;

    let compiled = compile(SRC);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let signal = DrainSignal::new();
    signal.request();
    let mut vm = drainable_vm(&core, &network, &signal);

    let result = vm.call(&named_hash(&compiled, "main"), Vec::new());
    signal.mark_complete();
    match result {
        Err(VmError::UnhandledAbility { ability_id, method }) => {
            assert_eq!(ability_id, ambient_core::drain::ability_id());
            assert_eq!(method, ambient_core::drain::requested_method_key());
        }
        other => panic!("an unhandled drain must fault as UnhandledAbility, got {other:?}"),
    }
}

/// The drain deadline: a non-cooperative computation — a hot loop that
/// never reaches an interruptible perform — is hard-stopped at the VM's
/// next opcode boundary once the deadline expires, surfacing as the
/// distinct `HardStopped` fault a driver parks on (never restarts, per
/// the fault-budget precedent). Its `Drain::requested` arm never runs:
/// cleanup is cooperative-only.
#[test]
fn the_deadline_hard_stops_a_non_cooperative_loop() {
    const SRC: &str = r"
    fn inner(n: Number): Number {
      if (n == 0) { 0 } else { inner(n - 1) }
    }
    fn mid(n: Number): Number {
      if (n == 0) { 0 } else {
        inner(200);
        mid(n - 1)
      }
    }
    fn spin(n: Number): Number {
      if (n == 0) { 0 } else {
        mid(200);
        spin(n - 1)
      }
    }
    pub fn run(): () {
      ()
    }
    pub fn main(): Number {
      with { core::system::Drain::requested() => 42 } handle spin(200)
    }
    ";

    let compiled = compile(SRC);
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let core = runtime(&network);
    deploy(&core, &compiled);

    let signal = DrainSignal::new();
    let mut vm = drainable_vm(&core, &network, &signal);
    let main = named_hash(&compiled, "main");

    let computation = std::thread::spawn(move || vm.call(&main, Vec::new()));
    signal.request_with_deadline(Duration::from_millis(25));

    let result = computation.join().expect("computation thread completes");
    signal.mark_complete();
    assert_eq!(
        result,
        Err(VmError::HardStopped),
        "a loop that never reaches an interruptible perform must be hard-stopped"
    );
}
