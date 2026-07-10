//! Integration tests for the process runtime and live upgrade.
//!
//! Each test compiles real Ambient source against the platform prelude
//! and drives it through [`ProcessRuntime::deploy`] — the same path
//! `ambient run` and `ambient dev` use. Observation happens through a
//! collector-backed Stdio, runtime introspection (`whereis`,
//! `process_count`), and the event sink.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ambient_engine::compiler::{CompileOptions, CompiledModule, compile_module_with_options};
use ambient_engine::infer::check_module_with_registry;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::value::Value;
use ambient_engine::vm::Vm;
use ambient_platform::process::{
    DeployOutcome, ProcessEvent, ProcessRuntime, ProcessRuntimeConfig, functions_from_module,
};
use ambient_platform::stdio_natives_with_collector;

/// Compile a test program against a core + compiled `core::system`
/// registry — the same world `ambient run` checks against.
///
/// The programs name `core::system::*` abilities, `core::convert::to_string`,
/// and the primitives (`Number`/`String`) fully qualified or bare. Core and
/// `core::system` compile for real (their functions — the ability default
/// implementations included — link by hash, so the program needs their
/// linking table and their compiled functions merged into the deployable
/// module).
fn compile(src: &str) -> CompiledModule {
    let module = ambient_parser::parse(src).expect("test program parses");

    let mut registry = ModuleRegistry::new();
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
    let compiled = compile_module_with_options(
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

    // The deployable module is the program plus the core and platform
    // modules it links against.
    let mut merged = core_compiled;
    merged.merge(&platform_compiled);
    merged.merge(&compiled);
    merged
}

struct TestHost {
    runtime: Arc<ProcessRuntime>,
    output: Arc<Mutex<Vec<String>>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl TestHost {
    fn new() -> Self {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let collector = Arc::clone(&output);
        let vm_factory = Arc::new(move || {
            let mut vm = Vm::new();
            // Stubs first so every platform extern is bound; the real
            // collector-backed Stdio natives overwrite theirs (uuid-keyed).
            vm.register_natives(&ambient_platform::stub_natives());
            vm.register_natives(&stdio_natives_with_collector(Arc::clone(&collector)));
            vm
        });

        let event_log = Arc::clone(&events);
        let sink = Arc::new(move |event: &ProcessEvent| {
            let line = match event {
                ProcessEvent::Started { name, .. } => format!("started {name}"),
                ProcessEvent::Upgraded { name } => format!("upgraded {name}"),
                ProcessEvent::Stopped { name } => format!("stopped {name}"),
                ProcessEvent::Exited { name } => format!("exited {name}"),
                ProcessEvent::Crashed {
                    name, restarting, ..
                } => format!("crashed {name} restarting={restarting}"),
                ProcessEvent::InitFailed { name, .. } => format!("init-failed {name}"),
            };
            event_log.lock().expect("event lock").push(line);
        });

        let runtime = ProcessRuntime::new(ProcessRuntimeConfig {
            vm_factory,
            events: sink,
        });

        Self {
            runtime,
            output,
            events,
        }
    }

    fn deploy(&self, src: &str) -> DeployOutcome {
        let compiled = compile(src);
        let entry = *compiled
            .function_names
            .get("run")
            .expect("test program has a `run` entry");
        self.runtime
            .deploy(&functions_from_module(&compiled), &entry)
            .expect("deploy succeeds")
    }

    fn output(&self) -> Vec<String> {
        self.output.lock().expect("output lock").clone()
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().expect("events lock").clone()
    }

    /// Poll until the collected Stdio output satisfies `pred`.
    fn wait_for_output(&self, pred: impl Fn(&[String]) -> bool) {
        wait_until(|| pred(&self.output()));
    }

    fn send(&self, name: &str, msg: Value) {
        let pid = self.runtime.whereis(name).expect("process is live");
        self.runtime.send_user(pid, msg);
    }
}

fn wait_until(pred: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pred() {
        assert!(Instant::now() < deadline, "timed out waiting for condition");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

/// Messages flow through a reducer; state accumulates across reductions.
#[test]
fn spawn_send_and_reduce() {
    let host = TestHost::new();
    host.deploy(
        r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("acc", init, add);
          core::system::Process::send!(pid, 5);
          core::system::Process::send!(pid, 7);
        }
        fn init(): Number { 0 }
        fn add(total: Number, n: Number): Number with core::system::Stdio {
          let next = total + n;
          core::system::Stdio::out!("total " + core::convert::to_string(next));
          next
        }
        "#,
    );

    host.wait_for_output(|out| out.len() >= 2);
    assert_eq!(host.output(), vec!["total 5", "total 12"]);
}

/// A redeploy with a changed reducer swaps code at the message boundary
/// and keeps the process state — the core live-upgrade guarantee.
#[test]
fn upgrade_keeps_state() {
    let host = TestHost::new();
    let v1 = r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("acc", init, step);
        }
        fn init(): Number { 0 }
        fn step(total: Number, n: Number): Number with core::system::Stdio {
          let next = total + n;
          core::system::Stdio::out!("v1 " + core::convert::to_string(next));
          next
        }
        "#;
    // v2 differs only in `step`'s body — the process must keep its count.
    let v2 = r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("acc", init, step);
        }
        fn init(): Number { 0 }
        fn step(total: Number, n: Number): Number with core::system::Stdio {
          let next = total + n;
          core::system::Stdio::out!("v2 " + core::convert::to_string(next));
          next
        }
        "#;

    let first = host.deploy(v1);
    assert_eq!(first.started, vec![Arc::from("acc")]);

    host.send("acc", Value::Number(3.0));
    host.wait_for_output(|out| out.contains(&"v1 3".to_string()));

    let second = host.deploy(v2);
    assert_eq!(second.upgraded, vec![Arc::from("acc")]);
    assert!(second.started.is_empty());
    assert!(second.stopped.is_empty());

    host.send("acc", Value::Number(4.0));
    host.wait_for_output(|out| out.contains(&"v2 7".to_string()));
    assert_eq!(host.output(), vec!["v1 3", "v2 7"]);
}

/// Deploys reconcile the declared tree: identical declarations are
/// untouched, removed names stop, new names start.
#[test]
fn reconcile_stops_removed_and_starts_added() {
    let host = TestHost::new();
    let common = r#"
        fn init(): Number { 0 }
        fn keep(total: Number, n: Number): Number { total + n }
        "#;
    let v1 = format!(
        r#"
        pub fn run(): () with core::system::Process {{
          core::system::Process::spawn!("a", init, keep);
          core::system::Process::spawn!("b", init, keep);
        }}
        {common}
        "#
    );
    let v2 = format!(
        r#"
        pub fn run(): () with core::system::Process {{
          core::system::Process::spawn!("b", init, keep);
          core::system::Process::spawn!("c", init, keep);
        }}
        {common}
        "#
    );

    host.deploy(&v1);
    wait_until(|| host.runtime.process_count() == 2);

    let outcome = host.deploy(&v2);
    assert_eq!(outcome.started, vec![Arc::from("c")]);
    assert_eq!(outcome.stopped, vec![Arc::from("a")]);
    assert_eq!(outcome.unchanged, 1);

    wait_until(|| host.runtime.whereis("a").is_none());
    assert!(host.runtime.whereis("b").is_some());
    assert!(host.runtime.whereis("c").is_some());
}

/// A faulting reduction restarts the process with fresh state from init
/// (flat supervision), and the crash is reported.
#[test]
fn crash_restarts_with_fresh_state() {
    let host = TestHost::new();
    host.deploy(
        r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("fragile", init, step);
        }
        fn init(): Number { 0 }
        fn step(total: Number, n: Number): Number with core::system::Stdio, Exception {
          if n < 0 {
            Exception::throw!("boom");
            total
          } else {
            let next = total + n;
            core::system::Stdio::out!("at " + core::convert::to_string(next));
            next
          }
        }
        "#,
    );

    host.send("fragile", Value::Number(5.0));
    host.wait_for_output(|out| out.contains(&"at 5".to_string()));

    // Crash it, then verify the next reduction starts from fresh state.
    host.send("fragile", Value::Number(-1.0));
    host.send("fragile", Value::Number(2.0));
    host.wait_for_output(|out| out.contains(&"at 2".to_string()));

    assert_eq!(host.output(), vec!["at 5", "at 2"]);
    assert!(
        host.events()
            .contains(&"crashed fragile restarting=true".to_string()),
        "crash must be reported: {:?}",
        host.events()
    );
}

/// Processes spawned during reductions are dynamic: deploys neither
/// stop nor reconcile them, and their code stays pinned.
#[test]
fn dynamic_processes_survive_deploys_untouched() {
    let host = TestHost::new();
    let v1 = r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("parent", init, parent);
        }
        fn init(): Number { 0 }
        fn parent(total: Number, n: Number): Number with core::system::Process {
          let child = core::system::Process::spawn!("child", init, child_step);
          total
        }
        fn child_step(total: Number, n: Number): Number with core::system::Stdio {
          core::system::Stdio::out!("child v1");
          total
        }
        "#;
    // v2 removes the child spawn and changes nothing else the child
    // depends on. The dynamic child must survive, running v1 code.
    let v2 = r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("parent", init, parent);
        }
        fn init(): Number { 0 }
        fn parent(total: Number, n: Number): Number with core::system::Stdio {
          core::system::Stdio::out!("parent v2");
          total
        }
        fn child_step(total: Number, n: Number): Number with core::system::Stdio {
          core::system::Stdio::out!("child v2");
          total
        }
        "#;

    host.deploy(v1);
    // Trigger the dynamic spawn.
    host.send("parent", Value::Number(1.0));
    wait_until(|| host.runtime.whereis("child").is_some());

    let outcome = host.deploy(v2);
    assert!(
        !outcome.stopped.contains(&Arc::from("child")),
        "dynamic processes are not reconciled"
    );
    assert!(host.runtime.whereis("child").is_some());

    // The pinned child still runs its spawn-time (v1) code.
    host.send("child", Value::Number(0.0));
    host.wait_for_output(|out| out.contains(&"child v1".to_string()));
    assert!(!host.output().contains(&"child v2".to_string()));
}

/// Spawning a live name outside a deploy pass raises a catchable
/// exception.
#[test]
fn duplicate_spawn_is_a_catchable_exception() {
    let host = TestHost::new();
    host.deploy(
        r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("parent", init, parent);
        }
        fn init(): Number { 0 }
        fn parent(total: Number, n: Number): Number with core::system::Process, core::system::Stdio {
          spawn_child();
          with { Exception::throw(e) => core::system::Stdio::out!("dup caught") } handle spawn_child();
          total
        }
        fn spawn_child(): () with core::system::Process {
          let pid = core::system::Process::spawn!("child", init, child_step);
        }
        fn child_step(total: Number, n: Number): Number { total }
        "#,
    );

    host.send("parent", Value::Number(1.0));
    host.wait_for_output(|out| out.contains(&"dup caught".to_string()));
}

/// `Process::exit!` stops the process after the current reduction, and
/// `run`-style waiting observes the tree winding down.
#[test]
fn exit_stops_the_process() {
    let host = TestHost::new();
    host.deploy(
        r#"
        pub fn run(): () with core::system::Process {
          let pid = core::system::Process::spawn!("oneshot", init, step);
          core::system::Process::send!(pid, 1);
        }
        fn init(): Number { 0 }
        fn step(total: Number, n: Number): Number with core::system::Process, core::system::Stdio {
          core::system::Stdio::out!("handled");
          core::system::Process::exit!();
          total
        }
        "#,
    );

    host.wait_for_output(|out| out.contains(&"handled".to_string()));
    host.runtime.wait_all();
    assert_eq!(host.runtime.process_count(), 0);
    assert!(host.events().contains(&"exited oneshot".to_string()));
}
