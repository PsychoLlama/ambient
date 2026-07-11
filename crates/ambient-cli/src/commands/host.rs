//! The CLI's runtime host: platform wiring shared by `run` and `dev`.
//!
//! A [`RuntimeHost`] owns everything that outlives a single VM — the
//! tokio runtime, the shared network handle table, the function store
//! (Execute), and the process and task runtimes — and knows how to
//! build a fully wired VM for any computation. Both `ambient run` and
//! `ambient dev` are thin drivers over [`RuntimeHost::deploy`]: `run`
//! deploys once and waits for the process tree and task registry to
//! wind down; `dev` deploys again on every code change. The REPL is the
//! third driver, over [`RuntimeHost::deploy_incremental`]: every turn is
//! a deploy whose entry is not a full program declaration.
//!
//! Wiring is native registration, uuid-keyed: stubs first (every platform
//! extern answers with a catchable "not wired" exception), then the real
//! implementation sets, which overwrite the stubs they cover. Process
//! natives are installed per process VM by the [`ProcessRuntime`] itself;
//! task natives per task VM by the [`TaskRuntime`], which also wires the
//! interruptible drain natives every task depends on.

use std::sync::{Arc, Mutex};
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
    store: Arc<Mutex<Store>>,
    runtime: Arc<ProcessRuntime>,
    tasks: Arc<TaskRuntime>,
    /// Serializes deploy passes across frontends. The deploy core's own
    /// state is lock-safe, but the task registry's reconcile bracket
    /// (`begin_reconcile`/`finish_reconcile`) is one global declaration
    /// diff — a dev-loop deploy and a remote `Deploy::apply!` (which
    /// runs on a task's thread) interleaving would cross their brackets.
    deploy_lock: Arc<Mutex<()>>,
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
        // The remote-deploy native's hook is deferred: the sets must
        // exist before the runtimes (the VM factory captures them), but
        // the hook needs the runtimes. Filled below, once they exist.
        let deploy_slot: ambient_platform::DeployApplySlot = Arc::default();
        sets.push(ambient_platform::remote_deploy_natives(Arc::clone(
            &deploy_slot,
        )));
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

        let deploy_lock = Arc::new(Mutex::new(()));

        // Wire the remote-deploy hook: a received generation pack is a
        // full program declaration, so it takes the same declarative
        // pass as `ambient run`/`dev` — decode, recompute hashes from
        // bytes (`from_pack` materializes objects), deploy, render the
        // report for the wire. Errors become the perform's `Err` value:
        // a rejected deploy must not fault the serving task.
        let hook: ambient_platform::DeployApplyHook = {
            let store = Arc::clone(&store);
            let runtime = Arc::clone(&runtime);
            let tasks = Arc::clone(&tasks);
            let lock = Arc::clone(&deploy_lock);
            Arc::new(move |bytes, entry| {
                let pack = ambient_engine::store::Pack::decode(bytes)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let compiled = CompiledModule::from_pack(&pack)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let outcome = deploy_build(&lock, &store, &runtime, &tasks, &compiled, entry)
                    .map_err(|e| e.to_string())?;
                Ok(render_deploy_report(&outcome))
            })
        };
        let _ = deploy_slot.set(hook);

        Ok(Self {
            _tokio: tokio,
            store,
            runtime,
            tasks,
            deploy_lock,
        })
    }

    /// Deploy a build: install it as the current code generation and run
    /// `entry` as a reconciliation pass over the live process tree and
    /// task registry.
    pub fn deploy(&self, compiled: &CompiledModule, entry: &str) -> Result<HostDeployOutcome> {
        deploy_build(
            &self.deploy_lock,
            &self.store,
            &self.runtime,
            &self.tasks,
            compiled,
            entry,
        )
    }

    /// Deploy a build *incrementally*: load, validate, swap, and run
    /// `entry` as a plain reconciliation body — spawns and ensures still
    /// register (and a re-spawn of a live name upgrades in place), but
    /// nothing is stopped or drained for being undeclared. This is the
    /// REPL's per-turn deploy: one turn is never a full declaration of
    /// the running program, so absence means "leave it running".
    ///
    /// Two deliberate asymmetries with [`Self::deploy`]:
    ///
    /// - The entry's own name binding is stripped from the generation.
    ///   An incremental entry is a synthetic per-turn body
    ///   (`repl::__repl_entry_N`), not a durable name — binding it would
    ///   root each turn's generation in the name table forever.
    /// - Task ensures are dynamic (`install_task_natives` with
    ///   `deploy: false`): they are not declarations, so no reconcile
    ///   pass brackets the entry and the outcome's task report is empty.
    pub fn deploy_incremental(
        &self,
        compiled: &CompiledModule,
        entry: &str,
    ) -> Result<HostDeployOutcome> {
        let _pass = self
            .deploy_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (entry_name, entry_hash) = resolve_entry(compiled, entry)?;

        match self.store.lock() {
            Ok(mut store) => store.add_module(compiled),
            Err(_) => bail!("function store lock poisoned"),
        }

        let mut functions = functions_from_module(compiled);
        if let Some(name) = &entry_name
            && let Some(generation) = Arc::get_mut(&mut functions)
        {
            generation.bindings.remove(name);
        }
        let processes = self
            .runtime
            .deploy_incremental(&functions, &entry_hash, |vm| {
                install_task_natives(vm, &self.tasks, false);
            })
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let retirement = self.runtime.deploy_core().retirement();
        Ok(HostDeployOutcome {
            processes,
            tasks: TaskReconcileOutcome::default(),
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

/// The full declarative deploy pass, shared by [`RuntimeHost::deploy`]
/// and the remote-deploy hook (which owns `Arc`s, not a `&RuntimeHost`):
/// install the build as the current code generation and run `entry` as a
/// reconciliation pass over the live process tree and task registry.
fn deploy_build(
    deploy_lock: &Mutex<()>,
    store: &Mutex<Store>,
    runtime: &Arc<ProcessRuntime>,
    tasks: &Arc<TaskRuntime>,
    compiled: &CompiledModule,
    entry: &str,
) -> Result<HostDeployOutcome> {
    // A poisoned lock means another frontend's deploy panicked; its pass
    // is over either way, so proceed rather than wedge every deploy.
    let _pass = deploy_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, entry_hash) = resolve_entry(compiled, entry)?;

    // Execute serves functions from the store; register every
    // generation so shipped code keeps resolving.
    match store.lock() {
        Ok(mut store) => store.add_module(compiled),
        Err(_) => bail!("function store lock poisoned"),
    }

    // One reconciliation pass over both deploy clients: the entry
    // VM carries the task natives as declarations, and the task
    // registry drains what the entry stopped declaring — only when
    // the entry ran to completion (an incomplete declaration must
    // not drain anything, mirroring the process reconciler).
    let functions = functions_from_module(compiled);
    tasks.begin_reconcile();
    let result = runtime.deploy_with(&functions, &entry_hash, |vm| {
        install_task_natives(vm, tasks, true);
    });
    let task_outcome = tasks.finish_reconcile(result.is_ok());
    let processes = result.map_err(|e| anyhow::anyhow!("{e}"))?;
    // Trace retirement after the full pass (drains included): the
    // report's reachable set is also the safety roots for purging
    // the on-disk store while the system runs.
    let retirement = runtime.deploy_core().retirement();
    Ok(HostDeployOutcome {
        processes,
        tasks: task_outcome,
        retirement,
    })
}

/// Render one deploy's outcome as the plain-text report `Deploy::apply!`
/// returns to the sender. Deterministic: every list is sorted (the name
/// diff already is), so tests and tooling can match on it.
fn render_deploy_report(outcome: &HostDeployOutcome) -> String {
    let names = &outcome.processes.names;
    let mut report = format!(
        "generation {}: {} rebound, {} retired, {} added, {} unchanged",
        outcome.processes.generation,
        names.rebound.len(),
        names.retired.len(),
        names.added.len(),
        names.unchanged,
    );
    for (label, list) in [
        ("rebound", &names.rebound),
        ("retired", &names.retired),
        ("added", &names.added),
    ] {
        for name in list.iter() {
            report.push_str(&format!("\n{label}: {name}"));
        }
    }
    for name in &outcome.tasks.started {
        report.push_str(&format!("\ntask started: {name}"));
    }
    for name in &outcome.tasks.drained {
        report.push_str(&format!("\ntask drained: {name}"));
    }
    for warning in &outcome.processes.warnings {
        report.push_str(&format!("\nwarning: {warning}"));
    }
    report
}

/// Resolve `entry` against a build's function names. Names in a merged
/// build carry their full canonical qualifier
/// (`workspace::<pkg>::main::run`), so the resolution order is: an exact
/// key; else any *module-path* key ending in `::{entry}` — uuid-rooted
/// dispatch symbols (`<uuid>::run`, e.g. Execute's own method impl) are
/// never entry points and are skipped; else, for the default entry, the
/// structurally-captured entry point (which matches no name key).
/// Returns the matched name key (when one exists) alongside the hash.
fn resolve_entry(
    compiled: &CompiledModule,
    entry: &str,
) -> Result<(Option<Arc<str>>, blake3::Hash)> {
    let suffix = format!("::{entry}");
    if let Some((name, hash)) = compiled.function_names.get_key_value(entry) {
        return Ok((Some(Arc::clone(name)), *hash));
    }
    if let Some((name, hash)) = compiled
        .function_names
        .iter()
        .find(|(name, _)| name.ends_with(&suffix) && !is_dispatch_symbol(name))
    {
        return Ok((Some(Arc::clone(name)), *hash));
    }
    if entry == "run"
        && let Some(hash) = compiled.entry_point
    {
        return Ok((None, hash));
    }
    Err(anyhow::anyhow!("entry function `{entry}` not found"))
}

/// Whether a store name is a content dispatch symbol (`<uuid>::method` for
/// ability default implementations and nominal-type methods) rather than a
/// module-qualified function name.
fn is_dispatch_symbol(name: &str) -> bool {
    name.split("::")
        .next()
        .is_some_and(|head| uuid::Uuid::parse_str(head).is_ok())
}
