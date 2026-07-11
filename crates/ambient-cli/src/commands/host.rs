//! The CLI's runtime host: platform wiring shared by `run` and `dev`.
//!
//! A [`RuntimeHost`] owns everything that outlives a single VM — the
//! tokio runtime, the shared network handle table, the function store
//! (Execute), and the process and task runtimes — and knows how to
//! build a fully wired VM for any computation. Both `ambient run` and
//! `ambient dev` are thin drivers over [`RuntimeHost::deploy`]: `run`
//! deploys once and waits for the process tree and task registry to
//! wind down; `dev` deploys again on every code change.
//!
//! Wiring is native registration, uuid-keyed: stubs first (every platform
//! extern answers with a catchable "not wired" exception), then the real
//! implementation sets, which overwrite the stubs they cover. Process
//! natives are installed per process VM by the [`ProcessRuntime`] itself;
//! task natives per task VM by the [`TaskRuntime`], which also wires the
//! interruptible drain natives every task depends on.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};

use ambient_engine::compiler::CompiledModule;
use ambient_engine::natives::NativeRegistry;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;
use ambient_platform::process::{
    DeployOutcome, EventSink, ProcessRuntime, ProcessRuntimeConfig, functions_from_module,
};
use ambient_platform::task::{
    TaskEventSink, TaskReconcileOutcome, TaskRuntime, TaskRuntimeConfig, install_task_natives,
};
use ambient_platform::{ExecuteConfig, NetworkState, StdioConfig, StdioSink};

/// How long a drained task gets to reach an interruptible perform
/// before it is hard-stopped and parked.
const TASK_DRAIN_DEADLINE: Duration = Duration::from_secs(5);

/// What one deploy did, across both deploy clients.
pub struct HostDeployOutcome {
    /// The process runtime's report (entry value, registry diff, name
    /// diff, generation id, warnings).
    pub processes: DeployOutcome,
    /// The task registry's report.
    pub tasks: TaskReconcileOutcome,
    /// The retirement trace run after the pass settled: which old
    /// generations retired, which are still pinned and by what (see
    /// `ref/live-upgrade.md`, "Retirement").
    pub retirement: ambient_platform::retire::RetirementReport,
}

/// Long-lived platform state plus the process and task runtimes.
pub struct RuntimeHost {
    /// Owns the reactor threads driving all network IO. Kept alive for
    /// the host's lifetime; unused otherwise.
    _tokio: tokio::runtime::Runtime,
    store: Arc<std::sync::Mutex<Store>>,
    runtime: Arc<ProcessRuntime>,
    tasks: Arc<TaskRuntime>,
}

impl RuntimeHost {
    /// Build the host: share one network table and one store across every
    /// future VM, and start an (empty) process runtime.
    ///
    /// `program_args` become `core::system::Env::args!()` for every VM
    /// this host builds (`ambient run` composes the program path plus the
    /// trailing args; `ambient dev` passes an empty vec — program args
    /// have no coherent meaning across reconciliation re-deploys).
    pub fn new(
        events: EventSink,
        task_events: TaskEventSink,
        program_args: Vec<String>,
    ) -> Result<Self> {
        let tokio = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;

        let network_state = Arc::new(NetworkState::new(tokio.handle().clone()));
        let store = Arc::new(std::sync::Mutex::new(Store::new()));
        let argv = Arc::new(program_args);

        // Log's default implementations perform Stdio, so both stream to
        // the same stdout for every VM this host builds.
        let sink = StdioSink::default();

        // Host policy for executed-by-hash code (Execute ability): shipped
        // code may print (Stdio, and Log through it) but gets no
        // FileSystem, Network, Time, Random, or recursive Execute — their
        // performs run the default implementations, whose extern calls
        // land on the stubs and raise catchable "not wired" exceptions.
        let exec_grants = {
            let sink = sink.clone();
            Arc::new(move |exec_vm: &mut Vm| {
                exec_vm.register_natives(&ambient_platform::stub_natives());
                exec_vm.register_natives(&ambient_platform::stdio_natives(
                    sink.clone(),
                    StdioConfig::default(),
                ));
            })
        };

        // One registry set, built once; each VM registration is
        // uuid-keyed, so the real sets overwrite the stubs they cover.
        let mut sets: Vec<NativeRegistry> = vec![
            ambient_platform::stub_natives(),
            ambient_platform::stdio_natives(sink, StdioConfig::default()),
            ambient_platform::time_natives(),
            ambient_platform::random_natives(),
            ambient_platform::fs_natives(),
            ambient_platform::env_natives(Arc::clone(&argv)),
            ambient_platform::network_natives(Arc::clone(&network_state)),
        ];
        sets.push(ambient_platform::execute_natives(ExecuteConfig {
            store: Arc::clone(&store),
            grants: Some(exec_grants as _),
        }));
        let sets = Arc::new(sets);

        let factory = {
            let sets = Arc::clone(&sets);
            Arc::new(move || {
                let mut vm = Vm::new();
                for set in sets.iter() {
                    vm.register_natives(set);
                }
                vm
            })
        };

        let runtime = ProcessRuntime::new(ProcessRuntimeConfig {
            vm_factory: factory,
            events,
        });
        let tasks = TaskRuntime::new(TaskRuntimeConfig {
            core: Arc::clone(runtime.deploy_core()),
            network: network_state,
            events: task_events,
            drain_deadline: TASK_DRAIN_DEADLINE,
        });

        Ok(Self {
            _tokio: tokio,
            store,
            runtime,
            tasks,
        })
    }

    /// Deploy a build: install it as the current code generation and run
    /// `entry` as a reconciliation pass over the live process tree and
    /// task registry.
    pub fn deploy(&self, compiled: &CompiledModule, entry: &str) -> Result<HostDeployOutcome> {
        // Function names in the merged build carry their full canonical
        // qualifier (`workspace::<pkg>::main::run`). Resolve `entry` in order:
        // an exact key; else any *module-path* key ending in `::{entry}` —
        // uuid-rooted dispatch symbols (`<uuid>::run`, e.g. Execute's own
        // method impl) are never entry points and are skipped; else, for
        // the default entry, the structurally-captured entry point.
        let suffix = format!("::{entry}");
        let entry_hash = compiled
            .function_names
            .get(entry)
            .or_else(|| {
                compiled
                    .function_names
                    .iter()
                    .find(|(name, _)| name.ends_with(&suffix) && !is_dispatch_symbol(name))
                    .map(|(_, hash)| hash)
            })
            .or_else(|| {
                (entry == "run")
                    .then_some(compiled.entry_point.as_ref())
                    .flatten()
            })
            .ok_or_else(|| anyhow::anyhow!("entry function `{entry}` not found"))?;

        // Execute serves functions from the store; register every
        // generation so shipped code keeps resolving.
        match self.store.lock() {
            Ok(mut store) => store.add_module(compiled),
            Err(_) => bail!("function store lock poisoned"),
        }

        // One reconciliation pass over both deploy clients: the entry
        // VM carries the task natives as declarations, and the task
        // registry drains what the entry stopped declaring — only when
        // the entry ran to completion (an incomplete declaration must
        // not drain anything, mirroring the process reconciler).
        let functions = functions_from_module(compiled);
        self.tasks.begin_reconcile();
        let result = self.runtime.deploy_with(&functions, entry_hash, |vm| {
            install_task_natives(vm, &self.tasks, true);
        });
        let tasks = self.tasks.finish_reconcile(result.is_ok());
        let processes = result.map_err(|e| anyhow::anyhow!("{e}"))?;
        // Trace retirement after the full pass (drains included): the
        // report's reachable set is also the safety roots for purging
        // the on-disk store while the system runs.
        let retirement = self.runtime.deploy_core().retirement();
        Ok(HostDeployOutcome {
            processes,
            tasks,
            retirement,
        })
    }

    /// The process runtime (for waiting / inspection).
    pub fn runtime(&self) -> &Arc<ProcessRuntime> {
        &self.runtime
    }

    /// The task runtime (for waiting / inspection).
    pub fn tasks(&self) -> &Arc<TaskRuntime> {
        &self.tasks
    }
}

/// Whether a store name is a content dispatch symbol (`<uuid>::method` for
/// ability default implementations and nominal-type methods) rather than a
/// module-qualified function name.
fn is_dispatch_symbol(name: &str) -> bool {
    name.split("::")
        .next()
        .is_some_and(|head| uuid::Uuid::parse_str(head).is_ok())
}
