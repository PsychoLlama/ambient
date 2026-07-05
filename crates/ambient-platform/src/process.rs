//! The process runtime: Erlang-inspired processes with live upgrade.
//!
//! A process is a named reducer — `init: () -> State` plus
//! `handler: (State, Msg) -> State` — driven by a mailbox. Each process
//! owns an OS thread and a VM, so blocking IO blocks only itself. The
//! runtime owns process state between reductions; that boundary is what
//! makes hot code replacement well-defined (see `ref/processes.md`).
//!
//! Code arrives in **generations**: content-addressed function tables
//! produced by compiling a package. A **deploy pass** runs an entry
//! function against the live registry in reconcile mode — `spawn!`
//! performs become declarations, diffed by content hash against the
//! running processes. Changed reducers swap code at their next message
//! boundary and keep their state; unchanged ones are untouched; removed
//! ones stop; new ones start. Dynamic processes (spawned outside deploy
//! passes) are pinned to their spawn-time code and never reconciled.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::bytecode::CompiledFunction;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::vm::Vm;

use crate::require;

/// A code generation: every compiled function of a build, shared.
pub type Functions = Arc<HashMap<blake3::Hash, Arc<CompiledFunction>>>;

/// Share a compiled module's function table as a code generation.
#[must_use]
pub fn functions_from_module(compiled: &CompiledModule) -> Functions {
    Arc::new(
        compiled
            .functions
            .iter()
            .map(|(hash, func)| (*hash, Arc::new(func.clone())))
            .collect(),
    )
}

/// Builds a base VM for a process: platform host handlers registered
/// (Stdio, Network, ...), no code loaded. The runtime layers the code
/// generation and the Process ability on top.
pub type VmFactory = Arc<dyn Fn() -> Vm + Send + Sync>;

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
    /// Base VM builder (platform handlers, no code).
    pub vm_factory: VmFactory,
    /// The resolved `Process` ability interface from the platform prelude.
    pub interface: AbilityInterface,
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
    generation: Functions,
    reconcile: Option<ReconcileState>,
}

/// The process runtime. One per program run; shared by every process VM.
pub struct ProcessRuntime {
    inner: Mutex<Inner>,
    /// Signaled whenever a process exits (for [`Self::wait_all`]).
    exited: Condvar,
    vm_factory: VmFactory,
    interface: AbilityInterface,
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
    /// Create a runtime with an initial (possibly empty) code generation.
    #[must_use]
    pub fn new(config: ProcessRuntimeConfig, generation: Functions) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                procs: HashMap::new(),
                names: HashMap::new(),
                next_pid: 1,
                generation,
                reconcile: None,
            }),
            exited: Condvar::new(),
            vm_factory: config.vm_factory,
            interface: config.interface,
            events: config.events,
        })
    }

    /// Run a deploy pass: install `functions` as the current generation
    /// and run `entry` in reconcile mode. This is both "start the
    /// program" (first call, empty registry) and "live upgrade" (every
    /// later call).
    ///
    /// # Errors
    ///
    /// Returns the runtime error if the entry function faults. Spawns
    /// performed before the fault stay live; reconciliation (stopping
    /// undeclared processes, staging code loads) is skipped because the
    /// declaration is incomplete.
    pub fn deploy(
        self: &Arc<Self>,
        functions: &Functions,
        entry: &blake3::Hash,
    ) -> Result<DeployOutcome, String> {
        {
            let mut inner = self.lock();
            inner.generation = Arc::clone(functions);
            inner.reconcile = Some(ReconcileState::default());
        }

        let mut vm = self.build_vm(functions, &ProcessContext { pid: 0, cell: None });
        let result = vm.call(entry, Vec::new());

        let mut inner = self.lock();
        let reconcile = inner.reconcile.take().unwrap_or_default();

        let value = match result {
            Ok(value) => value,
            Err(e) => return Err(vm.runtime_error(e).to_string()),
        };

        // Stop root processes the entry no longer declares. Dynamic
        // processes are pinned and never touched.
        let mut stopped = Vec::new();
        let to_stop: Vec<u64> = inner
            .procs
            .iter()
            .filter(|(_, p)| p.root && !reconcile.seen.contains(&p.name))
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
            value,
            started: reconcile.started,
            upgraded: reconcile.upgraded,
            stopped,
            unchanged: reconcile.unchanged,
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

    /// Build a VM wired for `ctx`: base platform handlers from the
    /// factory, the given generation loaded, Process ability bound.
    fn build_vm(self: &Arc<Self>, functions: &Functions, ctx: &ProcessContext) -> Vm {
        let mut vm = (self.vm_factory)();
        for func in functions.values() {
            vm.load_function_shared(Arc::clone(func));
        }
        register_process(&mut vm, &self.interface, self, ctx);
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

        let mut inner = self.lock();
        require_function(&inner.generation, &init, 0, "init")?;
        require_function(&inner.generation, &handler, 2, "handler")?;

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
        let generation = Arc::clone(&inner.generation);

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
                process_main(
                    &runtime,
                    pid,
                    &name,
                    init,
                    handler,
                    &receiver,
                    &cell,
                    &generation,
                );
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

/// Check that a value is callable with the given arity in a generation.
fn require_function(
    generation: &Functions,
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
    let Some(func) = generation.get(&hash) else {
        return Err(VmError::exception(format!(
            "Process.spawn: {role} references unknown code (hash not in this build)"
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
    generation: &Functions,
) {
    let mut vm = runtime.build_vm(
        generation,
        &ProcessContext {
            pid,
            cell: Some(Arc::clone(cell)),
        },
    );
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
                for func in functions.values() {
                    vm.load_function_shared(Arc::clone(func));
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
                    Ok(fresh) => state = fresh,
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

/// Register the Process ability handlers on a VM for a given identity.
#[allow(clippy::too_many_lines)]
fn register_process(
    vm: &mut Vm,
    ability: &AbilityInterface,
    runtime: &Arc<ProcessRuntime>,
    ctx: &ProcessContext,
) {
    // Process.spawn(name, init, handler) -> pid
    let rt = Arc::clone(runtime);
    let spawn_ctx = ctx.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "spawn"),
        Box::new(move |perform: &SuspendedAbility| {
            let name = match perform.args.first() {
                Some(Value::String(s)) => s.to_string(),
                other => {
                    return Err(VmError::exception(format!(
                        "Process.spawn: name must be a string, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let init = perform
                .args
                .get(1)
                .cloned()
                .ok_or_else(|| VmError::exception("Process.spawn: missing init argument"))?;
            let handler = perform
                .args
                .get(2)
                .cloned()
                .ok_or_else(|| VmError::exception("Process.spawn: missing handler argument"))?;
            let pid = rt.handle_spawn(&spawn_ctx, &name, init, handler)?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(pid as f64))
        }),
    );

    // Process.send(pid, msg) -> ()
    let rt = Arc::clone(runtime);
    vm.register_host_handler(
        ability.id,
        require(ability, "send"),
        Box::new(move |perform: &SuspendedAbility| {
            let pid = match perform.args.first() {
                Some(Value::Number(n)) => *n,
                other => {
                    return Err(VmError::exception(format!(
                        "Process.send: pid must be a number, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let msg = perform.args.get(1).cloned().unwrap_or(Value::Unit);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            rt.send_user(pid as u64, msg);
            Ok(Value::Unit)
        }),
    );

    // Process.send_named(name, msg) -> ()
    let rt = Arc::clone(runtime);
    vm.register_host_handler(
        ability.id,
        require(ability, "send_named"),
        Box::new(move |perform: &SuspendedAbility| {
            let name = match perform.args.first() {
                Some(Value::String(s)) => s.to_string(),
                other => {
                    return Err(VmError::exception(format!(
                        "Process.send_named: name must be a string, got {}",
                        other.map_or("nothing", |v| v.type_name())
                    )));
                }
            };
            let msg = perform.args.get(1).cloned().unwrap_or(Value::Unit);
            if let Some(pid) = rt.whereis(&name) {
                rt.send_user(pid, msg);
            }
            Ok(Value::Unit)
        }),
    );

    // Process.self_pid() -> pid (0 outside any process)
    let pid = ctx.pid;
    vm.register_host_handler(
        ability.id,
        require(ability, "self_pid"),
        Box::new(move |_perform: &SuspendedAbility| {
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(pid as f64))
        }),
    );

    // Process.whereis(name) -> pid (0 if none)
    let rt = Arc::clone(runtime);
    vm.register_host_handler(
        ability.id,
        require(ability, "whereis"),
        Box::new(move |perform: &SuspendedAbility| {
            let name = match perform.args.first() {
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

    // Process.exit() -> () — stop the calling process after this reduction.
    let exit_cell = ctx.cell.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "exit"),
        Box::new(move |_perform: &SuspendedAbility| match &exit_cell {
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
