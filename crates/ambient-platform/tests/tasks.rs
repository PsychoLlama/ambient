//! End-to-end tests for the task runtime (see ref/live-upgrade.md,
//! "Tasks").
//!
//! Each test compiles real Ambient source against core + `core::system`,
//! deploys it through a [`DeployRuntime`] with the entry VM wired as a
//! reconciliation pass (`install_task_natives(vm, tasks, true)`), and
//! observes the registry through cells and recorded [`TaskEvent`]s. The
//! contract under test:
//!
//! - A task body is **one bounded pass**: the runtime re-invokes it,
//!   re-resolving its deployed name each iteration, so a deploy that
//!   rebinds the body's name lands on the very next pass without the
//!   task restarting.
//! - `ensure` on a live name is a no-op; a root task a deploy pass
//!   stops declaring is drained, and its `Drain::requested` arm runs.
//! - Faulting passes restart until the consecutive-fault budget parks
//!   the task; a drain-deadline overrun hard-stops and parks it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ambient_ability::Value;
use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::vm::Vm;
use ambient_platform::TcpState;
use ambient_platform::deploy::{DeployRuntime, functions_from_module};
use ambient_platform::task::{
    TaskReconcileOutcome, TaskRuntime, TaskRuntimeConfig, install_task_natives,
};

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

/// Run the parse/check pipeline against core + `core::system` and return
/// the checker's diagnostics as strings (empty when the program is
/// well-typed). Used by the compile-time rejection tests, which assert the
/// checker — not just the runtime backstop — refuses a malformed task body.
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

    let checked = check_module_with_registry(module, &path, &registry);
    checked
        .errors
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

fn named_hash(compiled: &CompiledModule, name: &str) -> blake3::Hash {
    *compiled
        .function_names
        .get(name)
        .unwrap_or_else(|| panic!("test program defines `{name}`"))
}

/// Recorded task events, as debug strings (order preserved).
type Events = Arc<Mutex<Vec<String>>>;

/// A deploy core plus a task runtime wired against it — the production
/// shape.
struct Harness {
    /// Owns the reactor the network table needs; unused by these tests
    /// otherwise.
    _tokio: tokio::runtime::Runtime,
    core: Arc<DeployRuntime>,
    tasks: Arc<TaskRuntime>,
    events: Events,
}

fn harness(drain_deadline: Duration) -> Harness {
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let network = Arc::new(TcpState::new(tokio.handle().clone()));
    let factory_network = Arc::clone(&network);
    let core = Arc::new(DeployRuntime::new(Arc::new(move || {
        let mut vm = Vm::new();
        vm.register_natives(&ambient_platform::stub_natives());
        vm.register_natives(&ambient_platform::tcp_natives(Arc::clone(&factory_network)));
        vm
    })));
    let events: Events = Arc::default();
    let sink_events = Arc::clone(&events);
    let tasks = TaskRuntime::new(TaskRuntimeConfig {
        core: Arc::clone(&core),
        network,
        events: Arc::new(move |event| {
            #[allow(clippy::unwrap_used)]
            sink_events.lock().unwrap().push(format!("{event:?}"));
        }),
        drain_deadline,
    });
    Harness {
        _tokio: tokio,
        core,
        tasks,
        events,
    }
}

/// Run one deploy pass the way a host does: bracket the core deploy
/// with the task reconciler, wiring the entry VM's ensures as
/// declarations.
fn deploy(h: &Harness, compiled: &CompiledModule) -> TaskReconcileOutcome {
    h.tasks.begin_reconcile();
    let result = h.core.deploy(
        &functions_from_module(compiled),
        &named_hash(compiled, "run"),
        |vm| install_task_natives(vm, &h.tasks, true),
    );
    let outcome = h.tasks.finish_reconcile(result.is_ok());
    result.expect("test program deploys");
    outcome
}

/// Poll the cell table until `cell` holds at least `expected`.
fn await_at_least(h: &Harness, cell: &str, expected: f64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Value::Number(n)) = h.core.cells().get(cell)
            && n >= expected
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

/// Poll the retirement trace until `generation` has retired. Retirement
/// is permanent, so this is a monotonic readiness signal: the exact
/// condition the upgrade tests assert, not a cell-count proxy for it.
fn await_retired(h: &Harness, generation: u64) -> ambient_platform::retire::RetirementReport {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = h.core.retirement();
        if report.retired.contains(&generation) {
            return report;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for generation {generation} to retire: \
             current={:?}, pinned={:?}",
            report.current,
            report.pinned
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Poll until no task remains.
fn await_no_tasks(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.tasks.task_count() > 0 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the task registry to empty"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn recorded(h: &Harness) -> Vec<String> {
    #[allow(clippy::unwrap_used)]
    h.events.lock().unwrap().clone()
}

/// The runtime is the loop: a body that increments a cell and returns
/// is re-invoked, so the count climbs without any in-language
/// recursion (the language has no tail calls to spell one).
#[test]
fn the_runtime_reinvokes_the_task_body() {
    const SRC: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("count", () => 0);
      core::system::Task::ensure!("counter", tick)
    }

    fn tick(): () with core::system::State, core::system::Time {
      core::system::State::update!("count", (n: Number) => n + 1);
      core::system::Time::wait!(Duration::from_millis(2))
    }
    "#;

    let h = harness(Duration::from_secs(5));
    let outcome = deploy(&h, &compile(SRC));
    assert_eq!(outcome.started, vec![Arc::from("counter")]);
    assert!(outcome.drained.is_empty());

    await_at_least(&h, "count", 3.0);
    assert!(h.tasks.is_live("counter"));
    assert_eq!(h.tasks.task_count(), 1);

    h.tasks.drain_all();
    h.tasks.wait_all();
}

/// Redeploying an identical declaration is a no-op (`ensure` on a live
/// name), and a body rebinding lands on the next pass without the task
/// restarting: the runtime resolves the body's deployed name before
/// every iteration.
#[test]
fn a_task_picks_up_rebound_code_without_restarting() {
    const V1: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("mode", () => 0);
      core::system::Task::ensure!("worker", work)
    }

    fn work(): () with core::system::State, core::system::Time {
      core::system::State::set!("mode", 1);
      core::system::Time::wait!(Duration::from_millis(2))
    }
    "#;
    let v2 = V1.replace(r#""mode", 1"#, r#""mode", 2"#);

    let h = harness(Duration::from_secs(5));
    let first = deploy(&h, &compile(V1));
    assert_eq!(first.started, vec![Arc::from("worker")]);
    await_at_least(&h, "mode", 1.0);

    let second = deploy(&h, &compile(&v2));
    assert!(
        second.started.is_empty(),
        "ensure on a live name must not start a second task"
    );
    assert!(second.drained.is_empty());
    assert_eq!(second.unchanged, 1);

    // The next pass runs the rebound body — no restart, no re-ensure.
    await_at_least(&h, "mode", 2.0);
    assert_eq!(h.tasks.task_count(), 1);
    assert!(
        !recorded(&h).iter().any(|e| e.contains("Drained")),
        "the worker must never have wound down"
    );

    h.tasks.drain_all();
    h.tasks.wait_all();
}

/// A named task body is a resolution key, not pinned code: the per-pass
/// resolution stamps what actually runs, so once passes run the new
/// generation, nothing keeps the ensuring generation alive and it
/// retires — the task upgrades *through* the name.
#[test]
fn a_named_task_does_not_pin_the_generation_that_ensured_it() {
    const V1: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("count", () => 0);
      core::system::Task::ensure!("counter", tick)
    }

    fn tick(): () with core::system::State, core::system::Time {
      core::system::State::update!("count", (n: Number) => n + 1);
      core::system::Time::wait!(Duration::from_millis(2))
    }
    "#;
    let v2 = V1.replace("n + 1", "n + 2");

    let h = harness(Duration::from_secs(5));
    deploy(&h, &compile(V1));
    await_at_least(&h, "count", 1.0);

    deploy(&h, &compile(&v2));
    // Wait for the actual upgrade, not a cell-count proxy: `count` is
    // *not* a synchronization signal here. Generation 1's body keeps
    // ticking `count` up (by 1) the whole time the — comparatively slow
    // — `compile(&v2)` above is in flight, so by the time the swap lands
    // the count has long passed any fixed threshold on generation-1 code
    // alone. Poll the retirement trace instead: generation 1 retires the
    // moment the task's first post-swap pass re-stamps its resolution to
    // generation 2, and retirement is permanent, so this is monotonic.
    let report = await_retired(&h, 1);
    assert_eq!(report.current, Some(2));
    assert!(
        report.retired.contains(&1),
        "the task runs generation 2 code; nothing pins generation 1: {:?}",
        report.pinned
    );

    h.tasks.drain_all();
    h.tasks.wait_all();
}

/// A closure task body has no deployed name and stays pinned by design
/// (ensure on the live name is a no-op, so a redeploy does not replace
/// it either) — the retirement trace reports the task holding its
/// generation open.
#[test]
fn a_closure_task_body_pins_its_generation() {
    const V1: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("count", () => 0);
      core::system::Task::ensure!("held", () => beat())
    }

    fn beat(): () with core::system::State, core::system::Time {
      core::system::State::update!("count", (n: Number) => n + 1);
      core::system::Time::wait!(Duration::from_millis(2))
    }
    "#;
    let v2 = V1.replace("n + 1", "n + 2");

    let h = harness(Duration::from_secs(5));
    deploy(&h, &compile(V1));
    await_at_least(&h, "count", 1.0);

    deploy(&h, &compile(&v2));
    let report = h.core.retirement();
    assert_eq!(report.current, Some(2));
    let pinned = report
        .pinned
        .iter()
        .find(|generation| generation.id == 1)
        .expect("the closure body pins generation 1");
    assert!(
        pinned
            .pins
            .iter()
            .any(|pin| pin.root == ambient_platform::retire::RootOrigin::Task(Arc::from("held"))),
        "provenance should name the holding task: {:?}",
        pinned.pins
    );

    h.tasks.drain_all();
    h.tasks.wait_all();
}

const TICKER: &str = r#"
    use core::time::Duration;

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("count", () => 0);
      core::system::State::init!("goodbye", () => 0);
      core::system::Task::ensure!("ticker", ticker)
    }

    pub fn stop(): () with core::system::Task {
      core::system::Task::drain!("ticker")
    }

    fn ticker(): () with core::system::State, core::system::Time {
      with { core::system::Drain::requested() => core::system::State::set!("goodbye", 1) }
        handle tick()
    }

    fn tick(): () with core::system::State, core::system::Time {
      core::system::State::update!("count", (n: Number) => n + 1);
      core::system::Time::wait!(Duration::from_secs(3600))
    }
    "#;

/// A deploy that stops declaring a task drains it: the blocked wait is
/// interrupted, the task's own `Drain::requested` arm runs cleanup,
/// and the task winds down cleanly.
#[test]
fn an_undeclared_task_is_drained_through_its_cleanup_arm() {
    let h = harness(Duration::from_secs(5));
    deploy(&h, &compile(TICKER));
    await_at_least(&h, "count", 1.0);

    // The same program without the ensure: the entry stops declaring
    // the ticker.
    let without = TICKER.replace(r#"core::system::Task::ensure!("ticker", ticker)"#, "()");
    let outcome = deploy(&h, &compile(&without));
    assert_eq!(outcome.drained, vec![Arc::from("ticker")]);

    await_at_least(&h, "goodbye", 1.0);
    await_no_tasks(&h);
    let events = recorded(&h);
    assert!(
        events
            .iter()
            .any(|e| e.contains("Drained") && e.contains("cleanly: true")),
        "the ticker must report a clean drain, got {events:?}"
    );
    assert_eq!(
        h.core.cells().get("count").expect("cell exists"),
        Value::Number(1.0),
        "the unwind must discard the blocked pass: no further tick ran"
    );
}

/// `Task::drain!` from inside the language: any VM with the task
/// natives can drain by name, and draining an unknown name is a no-op.
#[test]
fn the_drain_perform_unwinds_a_named_task() {
    let h = harness(Duration::from_secs(5));
    let compiled = compile(TICKER);
    deploy(&h, &compiled);
    await_at_least(&h, "count", 1.0);

    let mut vm = h.core.build_vm();
    install_task_natives(&mut vm, &h.tasks, false);
    let stop = named_hash(&compiled, "stop");
    vm.call(&stop, Vec::new()).expect("stop runs");

    await_at_least(&h, "goodbye", 1.0);
    await_no_tasks(&h);

    // The name is free now: draining it again is a no-op.
    vm.call(&stop, Vec::new()).expect("stop is idempotent");
}

/// A faulting pass restarts (the retry re-resolves, so a later deploy
/// could fix it); the consecutive-fault budget parks the task.
#[test]
fn a_crash_looping_task_restarts_then_parks() {
    const SRC: &str = r#"
    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("attempts", () => 0);
      core::system::Task::ensure!("crasher", crash)
    }

    fn crash(): () with core::system::State, Exception {
      core::system::State::update!("attempts", (n: Number) => n + 1);
      Exception::throw!("boom")
    }
    "#;

    let h = harness(Duration::from_secs(5));
    deploy(&h, &compile(SRC));

    await_no_tasks(&h);
    assert_eq!(
        h.core.cells().get("attempts").expect("cell exists"),
        Value::Number(5.0),
        "the budget is five consecutive faults"
    );
    let events = recorded(&h);
    let faults: Vec<&String> = events.iter().filter(|e| e.contains("Faulted")).collect();
    assert_eq!(faults.len(), 5, "one event per faulting pass: {events:?}");
    assert!(faults[..4].iter().all(|e| e.contains("restarting: true")));
    assert!(
        faults[4].contains("restarting: false"),
        "the fifth fault parks: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e.contains("Drained")),
        "a parked task is not a drained task: {events:?}"
    );
}

/// A faulting task logs the *structured* trace, not a flattened message:
/// `TaskEvent::Faulted.error` is a `RuntimeError`, so a sink sees the
/// origin frame (its name and content hash) and the payload.
#[test]
fn a_task_fault_logs_a_hash_bearing_trace() {
    const SRC: &str = r#"
    pub fn run(): () with core::system::Task {
      core::system::Task::ensure!("boomer", boomer)
    }

    fn detonate(): () with Exception {
      Exception::throw!("kaboom in the task")
    }

    fn boomer(): () with Exception {
      detonate()
    }
    "#;

    let h = harness(Duration::from_secs(5));
    deploy(&h, &compile(SRC));
    await_no_tasks(&h);

    let events = recorded(&h);
    let fault = events
        .iter()
        .find(|e| e.contains("Faulted"))
        .unwrap_or_else(|| panic!("the task must fault: {events:?}"));

    // Payload plus the structured origin frame (name + content hash) — the
    // debug form of a `RuntimeError`, not a pre-rendered string.
    assert!(
        fault.contains("kaboom in the task"),
        "the fault must carry the payload: {fault}"
    );
    assert!(
        fault.contains("stack_trace") && fault.contains("function_hash"),
        "the fault must carry the structured trace: {fault}"
    );
    assert!(
        fault.contains("detonate"),
        "the trace must name the throw's origin frame: {fault}"
    );
}

/// A drained task that never reaches an interruptible perform is
/// hard-stopped at the deadline and parked — cleanup is
/// cooperative-only, so its `Drain::requested` arm never runs.
#[test]
fn a_non_cooperative_task_is_hard_stopped_at_the_drain_deadline() {
    const SRC: &str = r#"
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

    pub fn run(): () with core::system::State, core::system::Task {
      core::system::State::init!("passes", () => 0);
      core::system::Task::ensure!("spinner", spinner)
    }

    fn spinner(): () with core::system::State {
      core::system::State::update!("passes", (n: Number) => n + 1);
      spin(200);
      ()
    }
    "#;

    let h = harness(Duration::from_millis(25));
    deploy(&h, &compile(SRC));
    await_at_least(&h, "passes", 1.0);

    h.tasks.drain_all();
    await_no_tasks(&h);
    let events = recorded(&h);
    assert!(
        events
            .iter()
            // `Faulted.error` is now a structured `RuntimeError`; the
            // hard-stop shows as its `VmError::HardStopped` in the event's
            // debug form.
            .any(|e| e.contains("Faulted") && e.contains("HardStopped")),
        "the deadline must hard-stop the spinner: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e.contains("Drained")),
        "a hard stop parks; it is not a drain: {events:?}"
    );
}

/// The ensure contract is now enforced by the **checker**: a body that
/// takes parameters does not unify with `Task::ensure`'s `() -> () with E`
/// parameter, so the program is rejected before it ever compiles or
/// deploys. (`require_task_body` remains as a runtime backstop for
/// hand-written handlers and dynamic invocation paths.)
#[test]
fn ensure_rejects_a_parameterized_body() {
    const SRC: &str = r#"
    pub fn run(): () with core::system::Task {
      core::system::Task::ensure!("bad", takes_arg)
    }

    fn takes_arg(n: Number): () {
      ()
    }
    "#;

    let errors = check_errors(SRC);
    assert!(
        !errors.is_empty(),
        "a parameterized task body must be a compile error"
    );
}

/// A non-function task body (a plain `Number`) is likewise a checker
/// error: it cannot unify with the `() -> () with E` parameter type.
#[test]
fn ensure_rejects_a_non_function_body() {
    const SRC: &str = r#"
    pub fn run(): () with core::system::Task {
      core::system::Task::ensure!("bad", 42)
    }
    "#;

    let errors = check_errors(SRC);
    assert!(
        !errors.is_empty(),
        "a non-function task body must be a compile error"
    );
}

/// An effectful, zero-parameter task body type-checks: `E` appears only in
/// parameter position, so the body may perform (here `Stdio`) without the
/// effect propagating to `Task::ensure`'s own row.
#[test]
fn ensure_accepts_an_effectful_body() {
    const SRC: &str = r#"
    pub fn run(): () with core::system::Task {
      core::system::Task::ensure!("chatter", chatter)
    }

    fn chatter(): () with core::system::Stdio {
      core::system::Stdio::out!("tick")
    }
    "#;

    let errors = check_errors(SRC);
    assert!(
        errors.is_empty(),
        "an effectful zero-arg task body must type-check: {errors:?}"
    );
}
