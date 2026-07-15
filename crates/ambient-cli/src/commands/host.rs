//! The CLI's runtime host: platform wiring shared by `run` and `dev`.
//!
//! A [`RuntimeHost`] owns everything that outlives a single VM — the
//! tokio runtime, the shared network handle table, the function store
//! (Execute), the deploy core, and the task runtime — and knows how to
//! build a fully wired VM for any computation. Both `ambient run` and
//! `ambient dev` are thin drivers over [`RuntimeHost::deploy`]: `run`
//! deploys once and waits for the task registry to wind down; `dev`
//! deploys again on every code change. The REPL is the third driver,
//! over [`RuntimeHost::deploy_incremental`]: every turn is a deploy
//! whose entry is not a full program declaration.
//!
//! Wiring is native registration, uuid-keyed: stubs first (every platform
//! extern answers with a catchable "not wired" exception), then the real
//! implementation sets, which overwrite the stubs they cover. Task
//! natives are installed per task VM by the [`TaskRuntime`], which also
//! wires the interruptible drain natives every task depends on.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use anyhow::{Result, bail};

use ambient_engine::compiler::CompiledModule;
use ambient_engine::natives::NativeRegistry;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;
use ambient_platform::deploy::{DeployReport, DeployRuntime, PlanReport, functions_from_module};
use ambient_platform::task::{
    TaskEventSink, TaskReconcileOutcome, TaskRuntime, TaskRuntimeConfig, install_task_natives,
};
use ambient_platform::{ExecuteConfig, StdioConfig, StdioSink, TcpState};

/// How long a drained task gets to reach an interruptible perform
/// before it is hard-stopped and parked.
const TASK_DRAIN_DEADLINE: Duration = Duration::from_secs(5);

/// What one deploy did.
pub struct HostDeployOutcome {
    /// The deploy core's report (entry value, name diff, generation id,
    /// warnings).
    pub report: DeployReport,
    /// The task registry's report.
    pub tasks: TaskReconcileOutcome,
    /// The retirement trace run after the pass settled: which old
    /// generations retired, which are still pinned and by what (see
    /// `ref/live-upgrade.md`, "Retirement").
    pub retirement: ambient_platform::retire::RetirementReport,
}

/// Long-lived platform state plus the deploy core and task runtime.
pub struct RuntimeHost {
    /// Owns the reactor threads driving all network IO. Kept alive for
    /// the host's lifetime; unused otherwise.
    _tokio: tokio::runtime::Runtime,
    store: Arc<Mutex<Store>>,
    core: Arc<DeployRuntime>,
    tasks: Arc<TaskRuntime>,
    /// Serializes deploy passes across frontends, and records which thread
    /// (if any) is inside a pass. The deploy core's own state is lock-safe,
    /// but the task registry's reconcile bracket
    /// (`begin_reconcile`/`finish_reconcile`) is one global declaration
    /// diff — a dev-loop deploy and a remote `Deploy::apply!` (which
    /// runs on a task's thread) interleaving would cross their brackets.
    /// The recorded holder is what lets the remote-deploy hook reject a
    /// deploy performed *from within* a deploy pass (see [`DeployLock`]).
    deploy_lock: Arc<DeployLock>,
}

impl RuntimeHost {
    /// Build the host: share one network table and one store across every
    /// future VM, and start an (empty) deploy core.
    ///
    /// `program_args` become `core::system::Env::args!()` for every VM
    /// this host builds (`ambient run` composes the program path plus the
    /// trailing args; `ambient dev` passes an empty vec — program args
    /// have no coherent meaning across reconciliation re-deploys).
    ///
    /// `stdio` is where every VM's `Stdio` (and `Log`, which performs
    /// `Stdio`) output lands: `ambient run`/`dev` pass an inheriting sink
    /// (real stdout/stderr); the REPL's in-process test harness passes a
    /// capturing sink so program output can be asserted on.
    pub fn new(
        task_events: TaskEventSink,
        stdio: StdioSink,
        program_args: Vec<String>,
    ) -> Result<Self> {
        let tokio = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;

        let network_state = Arc::new(TcpState::new(tokio.handle().clone()));
        let store = Arc::new(std::sync::Mutex::new(Store::new()));
        let argv = Arc::new(program_args);

        // Log's default implementations perform Stdio, so both stream to
        // the same sink for every VM this host builds.
        let sink = stdio;

        // Host policy for executed-by-hash code (Execute ability): shipped
        // code may print (Stdio, and Log through it) but gets no
        // FileSystem, Tcp, Time, Random, or recursive Execute — their
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
            ambient_platform::tcp_natives(Arc::clone(&network_state)),
        ];
        sets.push(ambient_platform::execute_natives(ExecuteConfig {
            store: Arc::clone(&store),
            grants: Some(exec_grants as _),
        }));
        // The remote-deploy native's hook is deferred: the sets must
        // exist before the runtimes (the VM factory captures them), but
        // the hook needs the runtimes. Filled below, once they exist.
        let deploy_slot: ambient_platform::DeployApplySlot = Arc::default();
        let deploy_plan_slot: ambient_platform::DeployPlanSlot = Arc::default();
        sets.push(ambient_platform::remote_deploy_natives(
            Arc::clone(&deploy_slot),
            Arc::clone(&deploy_plan_slot),
        ));
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

        let core = Arc::new(DeployRuntime::new(factory));
        let tasks = TaskRuntime::new(TaskRuntimeConfig {
            core: Arc::clone(&core),
            network: network_state,
            events: task_events,
            drain_deadline: TASK_DRAIN_DEADLINE,
        });

        let deploy_lock = Arc::new(DeployLock::new());

        // Wire the remote-deploy hook: a received generation pack is a
        // full program declaration, so it takes the same declarative
        // pass as `ambient run`/`dev` — decode, recompute hashes from
        // bytes (`from_pack` materializes objects), deploy, render the
        // report for the wire. Errors become the perform's `Err` value:
        // a rejected deploy must not fault the serving task.
        let hook: ambient_platform::DeployApplyHook = {
            let store = Arc::clone(&store);
            let core = Arc::clone(&core);
            let tasks = Arc::clone(&tasks);
            let lock = Arc::clone(&deploy_lock);
            Arc::new(move |bytes, entry| {
                // Reentrancy guard, checked *before* anything else (before
                // pack decode): a deploy pass runs its reconciliation entry
                // synchronously on the lock-holding thread, so if that entry
                // (or anything it calls) performs `Deploy::apply!`, this hook
                // runs on that same thread while it still holds `deploy_lock`.
                // Falling through to `deploy_build` would re-take the
                // non-reentrant deploy mutex on the thread already holding it
                // — a permanent deadlock. Reject it as the perform's `Err`
                // value instead (the hook's existing rejection-as-a-value
                // contract). A *different* thread's `apply!` is not held here:
                // it blocks on the lock in `deploy_build` and serializes, as
                // intended.
                if lock.held_by_current_thread() {
                    return Err("Deploy.apply: cannot deploy from within a deploy pass".to_string());
                }
                let pack = ambient_engine::store::Pack::decode(bytes)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let compiled = CompiledModule::from_pack(&pack)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let outcome = deploy_build(&lock, &store, &core, &tasks, &compiled, entry)
                    .map_err(|e| e.to_string())?;
                Ok(render_deploy_report(&outcome))
            })
        };
        let _ = deploy_slot.set(hook);

        // Wire the remote-plan hook: a dry run of the same declarative pass.
        // Decode, recompute hashes from bytes, resolve the entry, and hand
        // the generation to the deploy core's `plan` — which loads inertly,
        // validates, and computes the would-be diff without swapping,
        // recording, or running the entry. It takes no deploy lock (no
        // reconcile bracket, no swap), so unlike `apply` it has no
        // reentrancy guard: planning from inside a deploy pass is fine, and
        // a plan never touches the store (Execute serves committed code, and
        // a dry run commits nothing). A malformed pack or an unresolvable
        // entry is the perform's `Err`; a validation failure is a successful
        // plan whose report names the problems.
        let plan_hook: ambient_platform::DeployPlanHook = {
            let core = Arc::clone(&core);
            Arc::new(move |bytes, entry| {
                let pack = ambient_engine::store::Pack::decode(bytes)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let compiled = CompiledModule::from_pack(&pack)
                    .map_err(|e| format!("invalid generation pack: {e}"))?;
                let (_, entry_hash) = resolve_entry(&compiled, entry).map_err(|e| e.to_string())?;
                let functions = functions_from_module(&compiled);
                let report = core.plan(&functions, &entry_hash);
                Ok(render_plan_report(&report))
            })
        };
        let _ = deploy_plan_slot.set(plan_hook);

        Ok(Self {
            _tokio: tokio,
            store,
            core,
            tasks,
            deploy_lock,
        })
    }

    /// Deploy a build: install it as the current code generation and run
    /// `entry` as a reconciliation pass over the live task registry.
    pub fn deploy(&self, compiled: &CompiledModule, entry: &str) -> Result<HostDeployOutcome> {
        deploy_build(
            &self.deploy_lock,
            &self.store,
            &self.core,
            &self.tasks,
            compiled,
            entry,
        )
    }

    /// Deploy a build *incrementally*: load, validate, swap, and run
    /// `entry` as a plain reconciliation body — ensures still register
    /// (as dynamic tasks), but nothing is drained for being undeclared.
    /// This is the REPL's per-turn deploy: one turn is never a full
    /// declaration of the running program, so absence means "leave it
    /// running".
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
        let _pass = self.deploy_lock.acquire();
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
        let report = self
            .core
            .deploy(&functions, &entry_hash, |vm| {
                install_task_natives(vm, &self.tasks, false);
            })
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let retirement = self.core.retirement();
        Ok(HostDeployOutcome {
            report,
            tasks: TaskReconcileOutcome::default(),
            retirement,
        })
    }

    /// The task runtime (for waiting / inspection).
    pub fn tasks(&self) -> &Arc<TaskRuntime> {
        &self.tasks
    }
}

/// The deploy serialization lock, plus the identity of the thread inside
/// the current pass.
///
/// A plain `Mutex<()>` serializes passes correctly but cannot answer the
/// one question the remote-deploy hook must ask: *am I already inside a
/// deploy pass on this very thread?* A reconciliation entry runs
/// synchronously on the lock-holding thread, so an entry that performs
/// `Deploy::apply!` re-enters the deploy machinery on that same thread —
/// re-taking a non-reentrant `Mutex` it already holds would deadlock every
/// future deploy. Recording the holder's [`ThreadId`] lets the hook detect
/// that case and reject it as a value instead.
///
/// Soundness: only the holding thread can observe "holder == me" truthfully
/// — it cannot race with itself, and the holder is set before the entry
/// runs and cleared before the gate is released (guard drop order). A
/// *different* thread may read a stale holder, but its only reaction is to
/// block on `gate`, which is exactly the serialization we want.
struct DeployLock {
    /// The serialization gate: held for the whole duration of a pass.
    gate: Mutex<()>,
    /// The thread currently inside a pass, or `None` when idle.
    holder: Mutex<Option<ThreadId>>,
}

impl DeployLock {
    /// An idle lock: no pass in progress, no holder.
    fn new() -> Self {
        Self {
            gate: Mutex::new(()),
            holder: Mutex::new(None),
        }
    }

    /// Enter a deploy pass: take the gate (blocking while another thread
    /// holds it) and record this thread as the holder until the returned
    /// guard drops.
    ///
    /// A poisoned lock means another frontend's deploy panicked; its pass
    /// is over either way, so we recover the guard rather than wedge every
    /// future deploy.
    fn acquire(&self) -> DeployPass<'_> {
        let gate = self.gate.lock().unwrap_or_else(PoisonError::into_inner);
        *self.holder.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread::current().id());
        DeployPass {
            lock: self,
            _gate: gate,
        }
    }

    /// Whether the current thread is the one inside the active pass — the
    /// reentrancy check the remote-deploy hook runs before doing anything.
    fn held_by_current_thread(&self) -> bool {
        *self.holder.lock().unwrap_or_else(PoisonError::into_inner) == Some(thread::current().id())
    }
}

/// An active deploy pass. Holds the gate for its lifetime and clears the
/// recorded holder on drop.
struct DeployPass<'a> {
    lock: &'a DeployLock,
    /// The gate guard. Declared last so it drops *after* [`Drop`] clears the
    /// holder — a thread that then acquires the gate always finds the holder
    /// already cleared.
    _gate: std::sync::MutexGuard<'a, ()>,
}

impl Drop for DeployPass<'_> {
    fn drop(&mut self) {
        *self
            .lock
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The full declarative deploy pass, shared by [`RuntimeHost::deploy`]
/// and the remote-deploy hook (which owns `Arc`s, not a `&RuntimeHost`):
/// install the build as the current code generation and run `entry` as a
/// reconciliation pass over the live task registry.
fn deploy_build(
    deploy_lock: &DeployLock,
    store: &Mutex<Store>,
    core: &Arc<DeployRuntime>,
    tasks: &Arc<TaskRuntime>,
    compiled: &CompiledModule,
    entry: &str,
) -> Result<HostDeployOutcome> {
    // Acquire the deploy pass. Blocks while another *thread* is mid-pass
    // (intended serialization). The remote-deploy hook has already rejected
    // a same-thread reentrant `apply!` before reaching here, so this never
    // deadlocks on the current thread.
    let _pass = deploy_lock.acquire();
    let (_, entry_hash) = resolve_entry(compiled, entry)?;

    // Execute serves functions from the store; register every
    // generation so shipped code keeps resolving.
    match store.lock() {
        Ok(mut store) => store.add_module(compiled),
        Err(_) => bail!("function store lock poisoned"),
    }

    // One reconciliation pass: the entry VM carries the task natives as
    // declarations, and the task registry drains what the entry stopped
    // declaring — only when the entry ran to completion (an incomplete
    // declaration must not drain anything).
    let functions = functions_from_module(compiled);
    tasks.begin_reconcile();
    let result = core.deploy(&functions, &entry_hash, |vm| {
        install_task_natives(vm, tasks, true);
    });
    let task_outcome = tasks.finish_reconcile(result.is_ok());
    let report = result.map_err(|e| anyhow::anyhow!("{e}"))?;
    // Trace retirement after the full pass (drains included): the
    // report's reachable set is also the safety roots for purging
    // the on-disk store while the system runs.
    let retirement = core.retirement();
    Ok(HostDeployOutcome {
        report,
        tasks: task_outcome,
        retirement,
    })
}

/// Render one deploy's outcome as the plain-text report `Deploy::apply!`
/// returns to the sender. Deterministic: every list is sorted (the name
/// diff already is), so tests and tooling can match on it.
fn render_deploy_report(outcome: &HostDeployOutcome) -> String {
    let names = &outcome.report.names;
    let mut report = format!(
        "generation {}: {} rebound, {} retired, {} added, {} unchanged",
        outcome.report.generation,
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
    for warning in &outcome.report.warnings {
        report.push_str(&format!("\nwarning: {warning}"));
    }
    report
}

/// Render a dry-run plan as the plain-text report `Deploy::plan!` returns.
/// Mirrors [`render_deploy_report`]'s shape and sorting, but generation-
/// less (a plan records nothing) and with no task/warning lines (a plan
/// never reconciles): a header, the per-name diff lines, then any
/// validation problems the plan *would* be rejected on.
fn render_plan_report(report: &PlanReport) -> String {
    let names = &report.names;
    let mut out = format!(
        "plan: {} rebound, {} retired, {} added, {} unchanged",
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
            out.push_str(&format!("\n{label}: {name}"));
        }
    }
    for problem in &report.problems {
        out.push_str(&format!("\nwould be rejected: {problem}"));
    }
    out
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;

    use super::*;
    use crate::commands::compile_source;

    /// A reconciliation entry that performs `Deploy::apply!` re-enters the
    /// deploy machinery on the very thread already holding the deploy lock
    /// (the entry runs synchronously inside the pass). Before the reentrancy
    /// guard this was a permanent deadlock of all future deploys; now the
    /// hook rejects it as the perform's `Err` value before touching the
    /// lock. The deploy runs on a spawned thread guarded by a channel
    /// timeout, so a regression fails fast instead of hanging the test
    /// process forever.
    #[test]
    fn deploy_within_a_deploy_pass_is_rejected_not_deadlocked() {
        // The entry performs `apply!` with garbage pack bytes on purpose:
        // the reentrancy check fires before the pack is decoded, so the
        // bytes never matter — only that the perform reaches the hook.
        let source = r#"
use core::system::Deploy;

pub fn run(): String with Deploy {
  match Deploy::apply!(Binary::from("not a real pack"), "run") {
    Ok(report) => report,
    Err(problem) => problem,
  }
}
"#;
        let compiled = compile_source(source, Path::new("reentrant_deploy.ab"))
            .expect("compile the reentrant-deploy program");

        let events: TaskEventSink = Arc::new(|_: &ambient_platform::TaskEvent| {});
        let stdio = StdioSink::new(Arc::new(|_: &str| {}), Arc::new(|_: &str| {}));
        let host = RuntimeHost::new(events, stdio, Vec::new()).expect("build host");

        // Run the deploy off-thread and time it out: a regression deadlocks
        // inside `deploy`, and we want a fast failure, not a hung process.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let value = host
                .deploy(&compiled, "run")
                .expect("deploy entry ran to completion")
                .report
                .value;
            let _ = tx.send(ambient_engine::format::format_value(&value));
        });

        let rendered = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("reentrant Deploy::apply! deadlocked instead of returning an Err");
        assert!(
            rendered.contains("cannot deploy from within a deploy pass"),
            "the entry should observe the reentrancy rejection as its \
             `apply!` Err, got: {rendered}"
        );
    }

    /// End-to-end `Deploy::plan!` through a real host: an in-language entry
    /// dry-runs a candidate generation pack and gets its report — proving
    /// the whole native → hook → deploy-core → render chain, and that a
    /// plan is safe from *inside* a deploy pass (the entry runs holding the
    /// deploy lock, yet `plan!` succeeds where `apply!` is rejected). The
    /// running system is untouched: the plan records no generation and
    /// binds no name, so the subsequent real deploy is generation 2 (not 3)
    /// and still sees the candidate's names as fresh additions.
    #[test]
    fn plan_runs_end_to_end_and_leaves_the_system_untouched() {
        // The candidate generation the entry will plan: a trivial program
        // whose user names are disjoint enough to read in the report.
        let candidate = compile_source(
            "pub fn run(): Number { 0 }
             pub fn alpha(): Number { 1 }
             pub fn beta(): Number { 2 }",
            Path::new("candidate.ab"),
        )
        .expect("compile the candidate generation");

        // Ship it to disk as a generation pack, the exact bytes `ambient
        // compile -o` writes, so the entry can read them back as a Binary.
        let pack_path = std::env::temp_dir().join(format!(
            "ambient-plan-e2e-{}-{:?}.pack",
            std::process::id(),
            thread::current().id()
        ));
        std::fs::write(&pack_path, candidate.to_pack().encode()).expect("write the candidate pack");

        // The driver: read the pack, `plan!` it (must succeed and report),
        // and `apply!` it (must be rejected from within the pass) — the
        // contrast the reentrancy guard draws.
        let driver_src = format!(
            r#"
use core::system::Deploy;
use core::system::FileSystem;

pub fn run(): String with Deploy, FileSystem {{
  match FileSystem::read_binary!("{path}") {{
    Ok(pack) => {{
      let planned = match Deploy::plan!(pack, "run") {{
        Ok(report) => report,
        Err(problem) => problem,
      }};
      let applied = match Deploy::apply!(pack, "run") {{
        Ok(report) => report,
        Err(problem) => problem,
      }};
      planned.concat("\n===\n").concat(applied)
    }},
    Err(e) => e,
  }}
}}
"#,
            path = pack_path.display()
        );
        let driver = compile_source(&driver_src, Path::new("plan_driver.ab"))
            .expect("compile the plan-driver program");

        let events: TaskEventSink = Arc::new(|_: &ambient_platform::TaskEvent| {});
        let stdio = StdioSink::new(Arc::new(|_: &str| {}), Arc::new(|_: &str| {}));
        let host = RuntimeHost::new(events, stdio, Vec::new()).expect("build host");

        // Generation 1: the driver. Its entry plans the candidate mid-pass.
        let first = host.deploy(&driver, "run").expect("driver deploys");
        assert_eq!(first.report.generation, 1);
        let rendered = ambient_engine::format::format_value(&first.report.value);

        // The plan half is a real plan report naming the candidate's adds.
        assert!(
            rendered.contains("plan:"),
            "the entry must observe a plan report, got: {rendered}"
        );
        assert!(
            rendered.contains("alpha") && rendered.contains("beta"),
            "the plan must report the candidate's added names, got: {rendered}"
        );
        // The apply half is the reentrancy rejection — plan succeeds where
        // apply cannot, from the very same in-pass call site.
        assert!(
            rendered.contains("cannot deploy from within a deploy pass"),
            "apply must still be rejected from within the pass, got: {rendered}"
        );

        // Untouched: the plan recorded no generation (so the real deploy of
        // the candidate is generation 2, not 3) and bound no name (so alpha
        // and beta are still fresh additions the real deploy makes).
        let second = host.deploy(&candidate, "run").expect("candidate deploys");
        assert_eq!(
            second.report.generation, 2,
            "the intervening plan must not have consumed a generation"
        );
        assert!(
            second
                .report
                .names
                .added
                .iter()
                .any(|n| n.contains("alpha"))
                && second.report.names.added.iter().any(|n| n.contains("beta")),
            "the plan must not have pre-bound the candidate's names, got added={:?}",
            second.report.names.added
        );

        let _ = std::fs::remove_file(&pack_path);
    }
}
