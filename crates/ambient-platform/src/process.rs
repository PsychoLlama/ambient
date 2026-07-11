//! The process runtime: Erlang-inspired processes with live upgrade.
//!
//! A process is a named reducer — `init: () -> State` plus
//! `handler: (State, Msg) -> State` — driven by a mailbox. Each process
//! owns an OS thread and a VM, so blocking IO blocks only itself. The
//! runtime owns process state between reductions; that boundary is what
//! makes hot code replacement well-defined (see `ref/processes.md`).
//!
//! Code arrives in **generations**: content-addressed function tables
//! produced by compiling a package. A **deploy pass** applies a generation
//! through the deploy core ([`crate::deploy`]) — load, validate, swap the
//! name table, run the entry — with the entry running against the live
//! registry in reconcile mode: `spawn!` performs become declarations,
//! diffed by content hash against the running processes. Changed reducers
//! swap code at their next message boundary and keep their state;
//! unchanged ones are untouched; removed ones stop; new ones start.
//! Dynamic processes (spawned outside deploy passes) are pinned to their
//! spawn-time code and never reconciled.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use ambient_ability::{Value, VmError};
use ambient_engine::vm::Vm;

use crate::deploy::{DeployRuntime, NameDiff};
pub use crate::deploy::{Functions, Generation, VmFactory, functions_from_module};
use crate::native_uuid;

/// Observable lifecycle events, for the embedder to log.
#[derive(Debug)]
pub enum ProcessEvent {
    /// A process was spawned.
    Started { name: Arc<str>, pid: u64 },
    /// A deploy pass staged new code for a live process (state kept).
    Upgraded { name: Arc<str> },
    /// A deploy pass stopped a process no longer declared.
    Stopped { name: Arc<str> },
    /// A process ended on its own (`Process::exit!` or mailbox closed).
    Exited { name: Arc<str> },
    /// A reduction faulted. `restarting` is false when the process
    /// exceeded its consecutive-fault budget and was parked.
    Crashed {
        name: Arc<str>,
        error: String,
        restarting: bool,
    },
    /// An init call faulted; the process is dead.
    InitFailed { name: Arc<str>, error: String },
}

/// Sink for [`ProcessEvent`]s.
pub type EventSink = Arc<dyn Fn(&ProcessEvent) + Send + Sync>;

/// Configuration for a [`ProcessRuntime`].
pub struct ProcessRuntimeConfig {
    /// Base VM builder (platform natives, no code).
    pub vm_factory: VmFactory,
    /// Lifecycle event sink.
    pub events: EventSink,
}

/// Result of a deploy pass.
pub struct DeployOutcome {
    /// The entry function's return value.
    pub value: Value,
    /// Names spawned fresh by this pass.
    pub started: Vec<Arc<str>>,
    /// Names whose code was swapped (state kept).
    pub upgraded: Vec<Arc<str>>,
    /// Names stopped because the entry no longer declared them.
    pub stopped: Vec<Arc<str>>,
    /// Declared names left completely untouched (hash-identical).
    pub unchanged: usize,
    /// The deploy core's exact name-table diff (item names, not process
    /// names — the fields above describe the process registry).
    pub names: NameDiff,
    /// The generation id the core recorded this deploy as.
    pub generation: u64,
    /// The core's deploy diagnostics (see `ref/live-upgrade.md`,
    /// "Deploy diagnostics").
    pub warnings: Vec<crate::retire::DeployWarning>,
}

/// Consecutive reduction faults before a process is parked.
const MAX_CONSECUTIVE_FAULTS: u32 = 5;

/// A message envelope. `Control` carries no payload — it exists to wake
/// a mailbox-blocked process so it notices staged code or a stop flag.
enum Envelope {
    User(Value),
    Control,
}

/// Staged work a process applies at its next mailbox wakeup.
#[derive(Default)]
struct Staged {
    /// Generations to load (additive; content addressing makes
    /// re-loading a known hash a no-op).
    generations: Vec<Functions>,
    /// New `(init, handler)` to swap in, keeping state.
    swap: Option<(Value, Value)>,
}

/// Per-process shared cell: stop flag plus staged code.
#[derive(Default)]
struct ProcCell {
    /// Stop at the next boundary (set by `exit!` or a deploy).
    stop: AtomicBool,
    /// Stopped by a deploy (suppresses the `Exited` event).
    stopped_by_deploy: AtomicBool,
    staged: Mutex<Option<Staged>>,
    /// The state value as of the last reduction boundary, published for
    /// the retirement trace (state may hold closures that pin their
    /// generation). Mid-reduction the next state is being built from
    /// this value plus the message — the boundary sample is exactly the
    /// "live frames" approximation the trace documents.
    published_state: Mutex<Option<Value>>,
}

impl ProcCell {
    fn publish_state(&self, state: &Value) {
        if let Ok(mut published) = self.published_state.lock() {
            *published = Some(state.clone());
        }
    }
}

/// Registry entry for a live process.
struct ProcHandle {
    name: Arc<str>,
    /// Spawned by a deploy pass (reconciled) vs dynamically (pinned).
    root: bool,
    sender: Sender<Envelope>,
    cell: Arc<ProcCell>,
    /// The latest intended init/handler — spawn-time values, updated
    /// when a deploy stages a swap. Deploy diffing compares against
    /// these.
    current_init: Value,
    current_handler: Value,
}

/// Bookkeeping for an in-flight deploy pass.
#[derive(Default)]
struct ReconcileState {
    seen: HashSet<Arc<str>>,
    started: Vec<Arc<str>>,
    upgraded: Vec<Arc<str>>,
    unchanged: usize,
}

struct Inner {
    procs: HashMap<u64, ProcHandle>,
    names: HashMap<Arc<str>, u64>,
    next_pid: u64,
    /// The most recently deployed generation. Staged (with the reducer
    /// swap) when a deploy re-declares a live process, so the swap never
    /// applies before its code is loaded in that process's VM.
    generation: Functions,
    reconcile: Option<ReconcileState>,
}

/// The process runtime. One per program run; shared by every process VM.
pub struct ProcessRuntime {
    inner: Mutex<Inner>,
    /// Signaled whenever a process exits (for [`Self::wait_all`]).
    exited: Condvar,
    /// The deploy core: owns the loaded object stores and the atomic
    /// name table; every process VM is built from it. `Arc`-shared so
    /// sibling clients (the task runtime) resolve against the same
    /// tables.
    core: Arc<DeployRuntime>,
    events: EventSink,
}

/// Identity of the VM being wired: a process (with its cell) or a
/// deploy/entry context (pid 0, spawns are declarations).
#[derive(Clone)]
struct ProcessContext {
    pid: u64,
    cell: Option<Arc<ProcCell>>,
}

impl ProcessContext {
    fn is_deploy(&self) -> bool {
        self.cell.is_none()
    }
}

impl ProcessRuntime {
    /// Create a runtime with an empty deploy core (nothing loaded, no
    /// names bound); the first [`Self::deploy`] starts the program. The
    /// registry is registered as a trace-root provider on the core:
    /// every live process contributes its reducers and last-published
    /// state to retirement tracing.
    #[must_use]
    pub fn new(config: ProcessRuntimeConfig) -> Arc<Self> {
        let runtime = Arc::new(Self {
            inner: Mutex::new(Inner {
                procs: HashMap::new(),
                names: HashMap::new(),
                next_pid: 1,
                generation: Arc::new(Generation::default()),
                reconcile: None,
            }),
            exited: Condvar::new(),
            core: Arc::new(DeployRuntime::new(config.vm_factory)),
            events: config.events,
        });
        let weak = Arc::downgrade(&runtime);
        runtime.core.register_roots(Arc::new(move || {
            weak.upgrade().map_or_else(Vec::new, |rt| rt.trace_roots())
        }));
        runtime
    }

    /// Every live process's contribution to the retirement trace: its
    /// current reducers (spawn-time values, updated when a deploy stages
    /// a swap — so a pinned dynamic process correctly pins its
    /// generation) and its state as of the last reduction boundary.
    /// Known hole, documented in [`crate::retire`]: values inside
    /// undelivered mailbox messages are invisible until reduced into
    /// state.
    fn trace_roots(&self) -> Vec<crate::retire::Root> {
        use crate::retire::{Root, RootOrigin};
        let inner = self.lock();
        let mut roots = Vec::new();
        for proc in inner.procs.values() {
            let origin = RootOrigin::Process(Arc::clone(&proc.name));
            roots.push(Root {
                origin: origin.clone(),
                value: proc.current_init.clone(),
            });
            roots.push(Root {
                origin: origin.clone(),
                value: proc.current_handler.clone(),
            });
            let state = proc
                .cell
                .published_state
                .lock()
                .ok()
                .and_then(|published| published.clone());
            if let Some(state) = state {
                roots.push(Root {
                    origin,
                    value: state,
                });
            }
        }
        roots
    }

    /// The deploy core this runtime is a client of (for name
    /// resolution, inspection, and sharing with sibling clients like
    /// the task runtime).
    #[must_use]
    pub fn deploy_core(&self) -> &Arc<DeployRuntime> {
        &self.core
    }

    /// Run a deploy pass: apply `functions` through the deploy core
    /// (load, validate, swap the name table) and run `entry` in reconcile
    /// mode. This is both "start the program" (first call, empty
    /// registry) and "live upgrade" (every later call).
    ///
    /// # Errors
    ///
    /// Returns the validation report if the core rejects the generation
    /// (nothing swapped, previous generation untouched), or the runtime
    /// error if the entry function faults. In the fault case, spawns
    /// performed before the fault stay live; reconciliation (stopping
    /// undeclared processes, staging code loads) is skipped because the
    /// declaration is incomplete.
    pub fn deploy(
        self: &Arc<Self>,
        functions: &Functions,
        entry: &blake3::Hash,
    ) -> Result<DeployOutcome, String> {
        self.deploy_with(functions, entry, |_| {})
    }

    /// [`Self::deploy`], with an extra wiring hook for the entry VM —
    /// how a host composes sibling deploy clients (the task runtime's
    /// `install_task_natives`) into the same reconciliation pass.
    ///
    /// # Errors
    ///
    /// See [`Self::deploy`].
    pub fn deploy_with(
        self: &Arc<Self>,
        functions: &Functions,
        entry: &blake3::Hash,
        wire: impl FnOnce(&mut Vm),
    ) -> Result<DeployOutcome, String> {
        self.deploy_pass(functions, entry, wire, true)
    }

    /// An *incremental* deploy: the same load/validate/swap/reconcile
    /// through the deploy core, but the entry is not a full declaration
    /// of the program — a spawn on a live name still upgrades in place,
    /// while processes the entry does not mention are left running
    /// (nothing is stopped for being undeclared). This is the REPL's
    /// per-turn deploy: a turn's absence means "leave it running", not
    /// "stop it".
    ///
    /// # Errors
    ///
    /// See [`Self::deploy`].
    pub fn deploy_incremental(
        self: &Arc<Self>,
        functions: &Functions,
        entry: &blake3::Hash,
        wire: impl FnOnce(&mut Vm),
    ) -> Result<DeployOutcome, String> {
        self.deploy_pass(functions, entry, wire, false)
    }

    /// The shared deploy pass. `declarative` is what separates the dev
    /// loop's full-program reconciliation from an incremental (REPL)
    /// turn: only a declarative pass stops root processes the entry no
    /// longer declares.
    fn deploy_pass(
        self: &Arc<Self>,
        functions: &Functions,
        entry: &blake3::Hash,
        wire: impl FnOnce(&mut Vm),
        declarative: bool,
    ) -> Result<DeployOutcome, String> {
        {
            let mut inner = self.lock();
            inner.generation = Arc::clone(functions);
            inner.reconcile = Some(ReconcileState::default());
        }

        let ctx = ProcessContext { pid: 0, cell: None };
        let report = self.core.deploy(functions, entry, |vm| {
            install_process_natives(vm, self, &ctx);
            wire(vm);
        });

        let mut inner = self.lock();
        let reconcile = inner.reconcile.take().unwrap_or_default();

        let report = match report {
            Ok(report) => report,
            Err(e) => return Err(e.to_string()),
        };

        // Stop root processes the entry no longer declares. Dynamic
        // processes are pinned and never touched, and an incremental
        // pass stops nothing — its entry is not a full declaration.
        let mut stopped = Vec::new();
        let to_stop: Vec<u64> = inner
            .procs
            .iter()
            .filter(|(_, p)| declarative && p.root && !reconcile.seen.contains(&p.name))
            .map(|(pid, _)| *pid)
            .collect();
        for pid in to_stop {
            if let Some(proc) = inner.procs.get(&pid) {
                proc.cell.stop.store(true, Ordering::SeqCst);
                proc.cell.stopped_by_deploy.store(true, Ordering::SeqCst);
                let _ = proc.sender.send(Envelope::Control);
                stopped.push(Arc::clone(&proc.name));
                let name = Arc::clone(&proc.name);
                // Free the name immediately so a later deploy can reuse
                // it; the thread removes its pid entry when it winds down.
                if inner.names.get(&name) == Some(&pid) {
                    inner.names.remove(&name);
                }
            }
        }

        // Every surviving process gets the new generation loaded
        // (additively) so values referencing new code — closures inside
        // messages — resolve even in pinned processes.
        for proc in inner.procs.values() {
            stage(&proc.cell, functions, None);
            let _ = proc.sender.send(Envelope::Control);
        }

        drop(inner);
        for name in &stopped {
            (self.events)(&ProcessEvent::Stopped {
                name: Arc::clone(name),
            });
        }

        Ok(DeployOutcome {
            value: report.value,
            started: reconcile.started,
            upgraded: reconcile.upgraded,
            stopped,
            unchanged: reconcile.unchanged,
            names: report.names,
            generation: report.generation,
            warnings: report.warnings,
        })
    }

    /// Number of live processes.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    #[must_use]
    pub fn process_count(&self) -> usize {
        self.lock().procs.len()
    }

    /// Block until every process has exited.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn wait_all(&self) {
        let mut inner = self.lock();
        while !inner.procs.is_empty() {
            inner = match self.exited.wait(inner) {
                Ok(guard) => guard,
                Err(_) => return,
            };
        }
    }

    /// Ask every process to stop at its next boundary.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn stop_all(&self) {
        let inner = self.lock();
        for proc in inner.procs.values() {
            proc.cell.stop.store(true, Ordering::SeqCst);
            proc.cell.stopped_by_deploy.store(true, Ordering::SeqCst);
            let _ = proc.sender.send(Envelope::Control);
        }
    }

    /// Deliver a message to a pid from outside the language (tests,
    /// embedders). Dropped if the pid is dead.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn send_user(&self, pid: u64, msg: Value) {
        let sender = self.lock().procs.get(&pid).map(|p| p.sender.clone());
        if let Some(sender) = sender {
            let _ = sender.send(Envelope::User(msg));
        }
    }

    /// The pid registered under a name, if any.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    #[must_use]
    pub fn whereis(&self, name: &str) -> Option<u64> {
        self.lock().names.get(name).copied()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        #[allow(clippy::unwrap_used)]
        self.inner.lock().unwrap()
    }

    /// Build a VM wired for `ctx`: the deploy core's view (base platform
    /// natives plus every deployed generation) with the `process_*`
    /// natives bound to this runtime and identity.
    fn build_vm(self: &Arc<Self>, ctx: &ProcessContext) -> Vm {
        let mut vm = self.core.build_vm();
        install_process_natives(&mut vm, self, ctx);
        vm
    }

    /// Spawn semantics for the `Process::spawn` perform.
    fn handle_spawn(
        self: &Arc<Self>,
        ctx: &ProcessContext,
        name: &str,
        init: Value,
        handler: Value,
    ) -> Result<u64, VmError> {
        if name.is_empty() {
            return Err(VmError::exception("Process.spawn: name must be non-empty"));
        }
        let name: Arc<str> = Arc::from(name);

        // Reducers may reference code from any deployed generation — old
        // hashes stay resident in the core — so a pinned process can spawn
        // with its own (superseded) refs.
        require_function(&self.core, &init, 0, "init")?;
        require_function(&self.core, &handler, 2, "handler")?;

        let mut inner = self.lock();

        // Deploy pass: an existing live name is a re-declaration.
        if ctx.is_deploy() && inner.reconcile.is_some() {
            if let Some(&pid) = inner.names.get(&name) {
                let generation = Arc::clone(&inner.generation);
                let Some(proc) = inner.procs.get_mut(&pid) else {
                    return Err(VmError::exception("Process.spawn: registry out of sync"));
                };
                let changed = proc.current_handler != handler || proc.current_init != init;
                if changed {
                    proc.current_init = init.clone();
                    proc.current_handler = handler.clone();
                    stage(&proc.cell, &generation, Some((init, handler)));
                    let _ = proc.sender.send(Envelope::Control);
                }
                let reconcile = inner
                    .reconcile
                    .as_mut()
                    .ok_or_else(|| VmError::exception("Process.spawn: reconcile state lost"))?;
                reconcile.seen.insert(Arc::clone(&name));
                if changed {
                    reconcile.upgraded.push(Arc::clone(&name));
                } else {
                    reconcile.unchanged += 1;
                }
                drop(inner);
                if changed {
                    (self.events)(&ProcessEvent::Upgraded { name });
                }
                return Ok(pid);
            }
        } else if inner.names.contains_key(&name) {
            return Err(VmError::exception(format!(
                "Process.spawn: a process named `{name}` is already live"
            )));
        }

        // Fresh spawn.
        let pid = inner.next_pid;
        inner.next_pid += 1;
        let (sender, receiver) = mpsc::channel();
        let cell = Arc::new(ProcCell::default());

        inner.procs.insert(
            pid,
            ProcHandle {
                name: Arc::clone(&name),
                root: ctx.is_deploy(),
                sender,
                cell: Arc::clone(&cell),
                current_init: init.clone(),
                current_handler: handler.clone(),
            },
        );
        inner.names.insert(Arc::clone(&name), pid);
        if let Some(reconcile) = inner.reconcile.as_mut()
            && ctx.is_deploy()
        {
            reconcile.seen.insert(Arc::clone(&name));
            reconcile.started.push(Arc::clone(&name));
        }
        drop(inner);

        (self.events)(&ProcessEvent::Started {
            name: Arc::clone(&name),
            pid,
        });

        let runtime = Arc::clone(self);
        let thread_name = format!("ambient-process-{name}");
        let spawned = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                process_main(&runtime, pid, &name, init, handler, &receiver, &cell);
            });
        if let Err(e) = spawned {
            self.remove_process(pid);
            return Err(VmError::exception(format!(
                "Process.spawn: failed to start thread: {e}"
            )));
        }

        Ok(pid)
    }

    /// Remove a process from the registry (called by its own thread as
    /// it winds down, or after a failed thread spawn).
    fn remove_process(&self, pid: u64) {
        let mut inner = self.lock();
        if let Some(proc) = inner.procs.remove(&pid)
            && inner.names.get(&proc.name) == Some(&pid)
        {
            inner.names.remove(&proc.name);
        }
        drop(inner);
        self.exited.notify_all();
    }
}

/// Merge staged work into a process cell.
fn stage(cell: &Arc<ProcCell>, generation: &Functions, swap: Option<(Value, Value)>) {
    #[allow(clippy::unwrap_used)]
    let mut staged = cell.staged.lock().unwrap();
    let entry = staged.get_or_insert_with(Staged::default);
    entry.generations.push(Arc::clone(generation));
    if swap.is_some() {
        entry.swap = swap;
    }
}

/// Check that a value is callable with the given arity in any deployed
/// generation.
fn require_function(
    core: &DeployRuntime,
    value: &Value,
    arity: u8,
    role: &str,
) -> Result<(), VmError> {
    let hash = match value {
        Value::Closure(c) => c.function_hash,
        Value::FunctionRef(h) => *h,
        other => {
            return Err(VmError::exception(format!(
                "Process.spawn: {role} must be a function, got {}",
                other.type_name()
            )));
        }
    };
    let Some(func) = core.lookup_function(&hash) else {
        return Err(VmError::exception(format!(
            "Process.spawn: {role} references unknown code (hash not deployed)"
        )));
    };
    if func.param_count != arity {
        return Err(VmError::exception(format!(
            "Process.spawn: {role} must take {arity} parameter(s), takes {}",
            func.param_count
        )));
    }
    Ok(())
}

/// Call a function-like value (closure or function ref) on a VM.
fn call_function_value(vm: &mut Vm, callee: &Value, args: Vec<Value>) -> Result<Value, String> {
    let result = match callee {
        Value::Closure(c) => vm.call_closure(&c.function_hash, args, c.environment.clone()),
        Value::FunctionRef(hash) => vm.call(hash, args),
        other => {
            return Err(format!("not callable: {}", other.type_name()));
        }
    };
    result.map_err(|e| vm.runtime_error(e).to_string())
}

/// A process's thread body: init, then reduce messages forever.
#[allow(clippy::too_many_arguments)]
fn process_main(
    runtime: &Arc<ProcessRuntime>,
    pid: u64,
    name: &Arc<str>,
    init: Value,
    handler: Value,
    receiver: &Receiver<Envelope>,
    cell: &Arc<ProcCell>,
) {
    let mut vm = runtime.build_vm(&ProcessContext {
        pid,
        cell: Some(Arc::clone(cell)),
    });
    let mut init = init;
    let mut handler = handler;

    let mut state = match call_function_value(&mut vm, &init, Vec::new()) {
        Ok(value) => value,
        Err(error) => {
            (runtime.events)(&ProcessEvent::InitFailed {
                name: Arc::clone(name),
                error,
            });
            runtime.remove_process(pid);
            return;
        }
    };
    cell.publish_state(&state);

    let mut consecutive_faults: u32 = 0;

    while !cell.stop.load(Ordering::SeqCst) {
        let Ok(envelope) = receiver.recv() else {
            break;
        };
        if cell.stop.load(Ordering::SeqCst) {
            break;
        }

        // Apply staged code before touching the next message: load new
        // generations (additive) and swap the reducer if a deploy
        // rebound this process. State carries over — that's the handoff.
        #[allow(clippy::unwrap_used)]
        let staged = cell.staged.lock().unwrap().take();
        if let Some(staged) = staged {
            for functions in &staged.generations {
                for func in functions.functions.values() {
                    vm.load_function_shared(Arc::clone(func));
                }
                for (hash, value) in &functions.values {
                    vm.load_value(*hash, value.clone());
                }
                for (hash, (uuid, param_count)) in &functions.natives {
                    vm.load_native(*hash, *uuid, *param_count);
                }
            }
            if let Some((new_init, new_handler)) = staged.swap {
                init = new_init;
                handler = new_handler;
            }
        }

        let Envelope::User(msg) = envelope else {
            continue;
        };

        match call_function_value(&mut vm, &handler, vec![state.clone(), msg]) {
            Ok(next_state) => {
                state = next_state;
                cell.publish_state(&state);
                consecutive_faults = 0;
            }
            Err(error) => {
                consecutive_faults += 1;
                let parked = consecutive_faults >= MAX_CONSECUTIVE_FAULTS;
                (runtime.events)(&ProcessEvent::Crashed {
                    name: Arc::clone(name),
                    error,
                    restarting: !parked,
                });
                if parked {
                    break;
                }
                // Supervision: restart in place with fresh state.
                match call_function_value(&mut vm, &init, Vec::new()) {
                    Ok(fresh) => {
                        state = fresh;
                        cell.publish_state(&state);
                    }
                    Err(error) => {
                        (runtime.events)(&ProcessEvent::InitFailed {
                            name: Arc::clone(name),
                            error,
                        });
                        break;
                    }
                }
            }
        }
    }

    let deploy_stopped = cell.stopped_by_deploy.load(Ordering::SeqCst);
    runtime.remove_process(pid);
    if !deploy_stopped {
        (runtime.events)(&ProcessEvent::Exited {
            name: Arc::clone(name),
        });
    }
}

/// Install the `process_*` native implementations on a VM for a given
/// identity. These overwrite the stubs (implementations are uuid-keyed):
/// the runtime's per-VM closures are what make `spawn`'s deploy semantics
/// and `self_pid`/`exit` identity-dependent.
#[allow(clippy::too_many_lines)]
fn install_process_natives(vm: &mut Vm, runtime: &Arc<ProcessRuntime>, ctx: &ProcessContext) {
    // process_spawn(name, init, handler) -> pid
    let rt = Arc::clone(runtime);
    let spawn_ctx = ctx.clone();
    vm.register_native_impl(
        native_uuid("process_spawn"),
        Arc::new(move |args: Vec<Value>| {
            let name = match args.first() {
                Some(Value::String(s)) => s.to_string(),
                other => {
                    return Err(VmError::exception(format!(
                        "Process.spawn: name must be a string, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let init = args
                .get(1)
                .cloned()
                .ok_or_else(|| VmError::exception("Process.spawn: missing init argument"))?;
            let handler = args
                .get(2)
                .cloned()
                .ok_or_else(|| VmError::exception("Process.spawn: missing handler argument"))?;
            let pid = rt.handle_spawn(&spawn_ctx, &name, init, handler)?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(pid as f64))
        }),
    );

    // process_send(pid, msg) -> ()
    let rt = Arc::clone(runtime);
    vm.register_native_impl(
        native_uuid("process_send"),
        Arc::new(move |args: Vec<Value>| {
            let pid = match args.first() {
                Some(Value::Number(n)) => *n,
                other => {
                    return Err(VmError::exception(format!(
                        "Process.send: pid must be a number, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let msg = args.get(1).cloned().unwrap_or(Value::Unit);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            rt.send_user(pid as u64, msg);
            Ok(Value::Unit)
        }),
    );

    // process_send_named(name, msg) -> ()
    let rt = Arc::clone(runtime);
    vm.register_native_impl(
        native_uuid("process_send_named"),
        Arc::new(move |args: Vec<Value>| {
            let name = match args.first() {
                Some(Value::String(s)) => s.to_string(),
                other => {
                    return Err(VmError::exception(format!(
                        "Process.send_named: name must be a string, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let msg = args.get(1).cloned().unwrap_or(Value::Unit);
            if let Some(pid) = rt.whereis(&name) {
                rt.send_user(pid, msg);
            }
            Ok(Value::Unit)
        }),
    );

    // process_self_pid() -> pid (0 outside any process)
    let pid = ctx.pid;
    vm.register_native_impl(
        native_uuid("process_self_pid"),
        Arc::new(move |_args: Vec<Value>| {
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(pid as f64))
        }),
    );

    // process_whereis(name) -> pid (0 if none)
    let rt = Arc::clone(runtime);
    vm.register_native_impl(
        native_uuid("process_whereis"),
        Arc::new(move |args: Vec<Value>| {
            let name = match args.first() {
                Some(Value::String(s)) => s.to_string(),
                other => {
                    return Err(VmError::exception(format!(
                        "Process.whereis: name must be a string, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(rt.whereis(&name).unwrap_or(0) as f64))
        }),
    );

    // process_exit() -> () — stop the calling process after this reduction.
    let exit_cell = ctx.cell.clone();
    vm.register_native_impl(
        native_uuid("process_exit"),
        Arc::new(move |_args: Vec<Value>| match &exit_cell {
            Some(cell) => {
                cell.stop.store(true, Ordering::SeqCst);
                Ok(Value::Unit)
            }
            None => Err(VmError::exception(
                "Process.exit: not inside a process (deploy passes cannot exit)",
            )),
        }),
    );
}

// The whole design rests on processes being movable to threads and
// values crossing between them.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Vm>();
    assert_send::<Value>();
};
