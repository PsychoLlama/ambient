//! The task runtime: named, supervised, drainable loops (see
//! `ref/live-upgrade.md`, "Tasks").
//!
//! A task is much less than a process: no mailbox, no reducer contract,
//! no message types — just "keep this body running under this name".
//! Each task owns an OS thread and a VM built from the shared
//! [`DeployRuntime`], wired drain-aware ([`install_drain_natives`]), so
//! the shared cell and handle tables are the only cross-task state.
//!
//! **The body is one bounded pass, not the loop.** Ambient has no tail
//! calls, so the ref doc's idiom — a loop that re-enters itself through
//! `Live::latest!` — cannot be spelled in the language yet. The runtime
//! is the loop: it re-invokes the body forever, resolving the body's
//! deployed name against the current generation before every iteration
//! (exactly the `live_latest` resolution). Edit the body — or anything
//! below it — and the next iteration runs the new code; the runtime
//! never swaps code mid-pass. A closure body has no deployed name and
//! stays pinned to its hash.
//!
//! Tasks are **reconciled by name** during deploy passes: the driving
//! host brackets the deploy with [`TaskRuntime::begin_reconcile`] /
//! [`TaskRuntime::finish_reconcile`], `ensure` on a live name is a
//! no-op, and a root task the entry stops declaring is drained — its
//! [`DrainSignal`] is requested with the runtime's deadline, the
//! current pass unwinds at its next interruptible perform, the nearest
//! `Drain::requested` arm runs cleanup, and the task winds down.
//!
//! Fault handling follows the process runtime's precedent: a faulting
//! pass is retried (the retry re-resolves, so a deploy can fix a
//! crash-looping task), and a task that faults too many times in a row
//! is parked. A hard stop ([`VmError::HardStopped`], the drain
//! deadline's backstop) always parks — restarting a hard-stopped VM
//! would just hard-stop again, its interrupt flag stays set.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ambient_ability::{Value, VmError};
use ambient_engine::vm::Vm;

use crate::deploy::DeployRuntime;
use crate::drain::{DrainSignal, install_drain_natives};
use crate::native_uuid;
use crate::network_state::NetworkState;

/// Observable lifecycle events, for the embedder to log.
#[derive(Debug)]
pub enum TaskEvent {
    /// A task was started.
    Started { name: Arc<str> },
    /// A drain was requested (a deploy stopped declaring the task, or
    /// `Task::drain!` named it); the name is free immediately.
    Draining { name: Arc<str> },
    /// A drained task wound down. `cleanly` is true when its final pass
    /// completed (a `Drain::requested` arm ran, or it finished between
    /// interruptible performs); false when the unwind surfaced
    /// unhandled or the pass faulted mid-drain.
    Drained { name: Arc<str>, cleanly: bool },
    /// A pass faulted. `restarting` is false when the task was parked —
    /// its consecutive-fault budget ran out, or it overran a drain
    /// deadline and was hard-stopped.
    Faulted {
        name: Arc<str>,
        error: String,
        restarting: bool,
    },
}

/// Sink for [`TaskEvent`]s.
pub type TaskEventSink = Arc<dyn Fn(&TaskEvent) + Send + Sync>;

/// Configuration for a [`TaskRuntime`].
pub struct TaskRuntimeConfig {
    /// The deploy core tasks build their VMs from and resolve their
    /// bodies against. Shared with every other client (the process
    /// runtime, the entry VM) so all see one name table and one cell
    /// table.
    pub core: Arc<DeployRuntime>,
    /// The shared network handle table — the interruptible natives
    /// need it to race blocked operations against the drain signal.
    pub network: Arc<NetworkState>,
    /// Lifecycle event sink.
    pub events: TaskEventSink,
    /// How long a drained task gets to reach an interruptible perform
    /// before it is hard-stopped and parked.
    pub drain_deadline: Duration,
}

/// Consecutive pass faults before a task is parked (the process
/// runtime's budget).
const MAX_CONSECUTIVE_FAULTS: u32 = 5;

/// Registry entry for a live task.
struct TaskHandle {
    name: Arc<str>,
    /// Ensured by a deploy pass (reconciled) vs dynamically (never
    /// drained implicitly).
    root: bool,
    signal: Arc<DrainSignal>,
    /// The ensure-time body value. For the retirement trace this is
    /// only the *fallback* root (before the first pass stamps a
    /// resolution, and for closure bodies, which really are the running
    /// code): a named body's spawn-time hash is a resolution key, and
    /// rooting it would pin the ensuring generation forever while the
    /// task runs ever-fresher code.
    body: Value,
    /// The hash the task's thread resolved for the pass it is running
    /// (or last ran) — the "live frames sampled at boundaries" half of
    /// the retirement trace: while a pass is in flight its frames can
    /// only hold code reachable from this hash plus already-rooted
    /// values (cells, registries, the name table).
    current_pass: Arc<Mutex<Option<blake3::Hash>>>,
}

/// Bookkeeping for an in-flight deploy pass.
#[derive(Default)]
struct ReconcileState {
    seen: HashSet<Arc<str>>,
    started: Vec<Arc<str>>,
    unchanged: usize,
}

/// What a deploy pass did to the task registry.
#[derive(Debug, Default)]
pub struct TaskReconcileOutcome {
    /// Names started fresh by this pass.
    pub started: Vec<Arc<str>>,
    /// Names being drained because the entry no longer declared them.
    pub drained: Vec<Arc<str>>,
    /// Declared names that were already live (ensure's no-op).
    pub unchanged: usize,
}

#[derive(Default)]
struct Inner {
    tasks: HashMap<u64, TaskHandle>,
    names: HashMap<Arc<str>, u64>,
    next_id: u64,
    reconcile: Option<ReconcileState>,
}

/// The task runtime: the named registry of drainable loops. One per
/// running system, a client of the shared [`DeployRuntime`].
pub struct TaskRuntime {
    inner: Mutex<Inner>,
    /// Signaled whenever a task winds down (for [`Self::wait_all`]).
    exited: Condvar,
    core: Arc<DeployRuntime>,
    network: Arc<NetworkState>,
    events: TaskEventSink,
    drain_deadline: Duration,
}

/// How one task ended, decided by its own thread.
enum Ending {
    Drained { cleanly: bool },
    Parked,
}

impl TaskRuntime {
    /// Create a runtime with an empty registry, registered as a
    /// trace-root provider on the shared deploy core (every live task —
    /// including ones winding down from a drain — contributes its
    /// current code to retirement tracing).
    #[must_use]
    pub fn new(config: TaskRuntimeConfig) -> Arc<Self> {
        let runtime = Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            exited: Condvar::new(),
            core: config.core,
            network: config.network,
            events: config.events,
            drain_deadline: config.drain_deadline,
        });
        let weak = Arc::downgrade(&runtime);
        runtime.core.register_roots(Arc::new(move || {
            weak.upgrade().map_or_else(Vec::new, |rt| rt.trace_roots())
        }));
        runtime
    }

    /// Every live task's contribution to the retirement trace: the hash
    /// its thread resolved for the current (or last) pass, or the
    /// ensure-time body value before a first pass has stamped one (and
    /// for closures, whose captured environment must be walked too).
    fn trace_roots(&self) -> Vec<crate::retire::Root> {
        use crate::retire::{Root, RootOrigin};
        let inner = self.lock();
        inner
            .tasks
            .values()
            .map(|task| {
                let origin = RootOrigin::Task(Arc::clone(&task.name));
                let stamped = task
                    .current_pass
                    .lock()
                    .ok()
                    .and_then(|current| *current)
                    .filter(|_| matches!(task.body, Value::FunctionRef(_)));
                match stamped {
                    Some(hash) => Root::from_hash(origin, hash),
                    None => Root {
                        origin,
                        value: task.body.clone(),
                    },
                }
            })
            .collect()
    }

    /// Number of live tasks (including ones still winding down from a
    /// drain).
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.lock().tasks.len()
    }

    /// Whether a task currently holds `name`.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    #[must_use]
    pub fn is_live(&self, name: &str) -> bool {
        self.lock().names.contains_key(name)
    }

    /// Block until every task has wound down.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn wait_all(&self) {
        let mut inner = self.lock();
        while !inner.tasks.is_empty() {
            inner = match self.exited.wait(inner) {
                Ok(guard) => guard,
                Err(_) => return,
            };
        }
    }

    /// Drain every task (shutdown): each unwinds at its next
    /// interruptible perform or is hard-stopped at the deadline.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn drain_all(&self) {
        let mut inner = self.lock();
        let names: Vec<Arc<str>> = inner.names.keys().cloned().collect();
        let drained: Vec<Arc<str>> = names
            .iter()
            .filter_map(|name| self.drain_locked(&mut inner, name))
            .collect();
        drop(inner);
        for name in drained {
            (self.events)(&TaskEvent::Draining { name });
        }
    }

    /// Start a deploy pass: `ensure` performs from now until
    /// [`Self::finish_reconcile`] are declarations, diffed against the
    /// live registry.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn begin_reconcile(&self) {
        self.lock().reconcile = Some(ReconcileState::default());
    }

    /// End a deploy pass. When `apply` is true (the entry ran to
    /// completion), root tasks the pass did not re-declare are drained;
    /// on an incomplete pass (`apply` false — validation rejected the
    /// deploy or the entry faulted) the declaration is incomplete, so
    /// nothing is drained, mirroring the process runtime.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn finish_reconcile(&self, apply: bool) -> TaskReconcileOutcome {
        let mut inner = self.lock();
        let reconcile = inner.reconcile.take().unwrap_or_default();
        let mut outcome = TaskReconcileOutcome {
            started: reconcile.started,
            drained: Vec::new(),
            unchanged: reconcile.unchanged,
        };
        if !apply {
            return outcome;
        }
        let undeclared: Vec<Arc<str>> = inner
            .names
            .iter()
            .filter(|(name, id)| {
                !reconcile.seen.contains(*name) && inner.tasks.get(id).is_some_and(|task| task.root)
            })
            .map(|(name, _)| Arc::clone(name))
            .collect();
        for name in undeclared {
            if let Some(name) = self.drain_locked(&mut inner, &name) {
                outcome.drained.push(name);
            }
        }
        outcome.drained.sort();
        drop(inner);
        for name in &outcome.drained {
            (self.events)(&TaskEvent::Draining {
                name: Arc::clone(name),
            });
        }
        outcome
    }

    /// Ensure semantics for the `Task::ensure` perform: start the task
    /// if no live one holds the name, no-op otherwise. `deploy` marks
    /// an ensure performed by a reconciliation entry — those register
    /// as declarations (and their tasks as roots).
    fn ensure(self: &Arc<Self>, deploy: bool, name: &str, body: Value) -> Result<(), VmError> {
        if name.is_empty() {
            return Err(VmError::exception("Task.ensure: name must be non-empty"));
        }
        require_task_body(&self.core, &body)?;
        let name: Arc<str> = Arc::from(name);

        let mut inner = self.lock();
        if deploy && let Some(reconcile) = inner.reconcile.as_mut() {
            reconcile.seen.insert(Arc::clone(&name));
        }
        if inner.names.contains_key(&name) {
            // Ensure on a live name is a no-op: the runtime never swaps
            // a task's code — freshness comes from the per-iteration
            // name resolution, not from re-declaring.
            if deploy && let Some(reconcile) = inner.reconcile.as_mut() {
                reconcile.unchanged += 1;
            }
            return Ok(());
        }

        let id = inner.next_id;
        inner.next_id += 1;
        let signal = DrainSignal::new();
        let current_pass = Arc::new(Mutex::new(None));
        inner.tasks.insert(
            id,
            TaskHandle {
                name: Arc::clone(&name),
                root: deploy,
                signal: Arc::clone(&signal),
                body: body.clone(),
                current_pass: Arc::clone(&current_pass),
            },
        );
        inner.names.insert(Arc::clone(&name), id);
        if deploy && let Some(reconcile) = inner.reconcile.as_mut() {
            reconcile.started.push(Arc::clone(&name));
        }
        drop(inner);

        (self.events)(&TaskEvent::Started {
            name: Arc::clone(&name),
        });

        let runtime = Arc::clone(self);
        let thread_name = format!("ambient-task-{name}");
        let spawned = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let ending = task_main(&runtime, &name, &body, &signal, &current_pass);
                signal.mark_complete();
                runtime.remove_task(id);
                if let Ending::Drained { cleanly } = ending {
                    (runtime.events)(&TaskEvent::Drained { name, cleanly });
                }
            });
        if let Err(e) = spawned {
            self.remove_task(id);
            return Err(VmError::exception(format!(
                "Task.ensure: failed to start thread: {e}"
            )));
        }
        Ok(())
    }

    /// Drain semantics for the `Task::drain` perform (and the
    /// reconciler): request the task's signal with the runtime's
    /// deadline and free the name immediately. No-op if no task holds
    /// the name.
    fn drain(&self, name: &str) {
        let mut inner = self.lock();
        let drained = self.drain_locked(&mut inner, name);
        drop(inner);
        if let Some(name) = drained {
            (self.events)(&TaskEvent::Draining { name });
        }
    }

    /// Request a drain on the task holding `name` and free the name.
    /// Returns the drained task's name; the caller emits the event
    /// after releasing the registry lock.
    fn drain_locked(&self, inner: &mut Inner, name: &str) -> Option<Arc<str>> {
        let id = inner.names.remove(name)?;
        let task = inner.tasks.get(&id)?;
        task.signal.request_with_deadline(self.drain_deadline);
        Some(Arc::clone(&task.name))
    }

    /// Remove a task from the registry (called by its own thread as it
    /// winds down, or after a failed thread spawn).
    fn remove_task(&self, id: u64) {
        let mut inner = self.lock();
        if let Some(task) = inner.tasks.remove(&id)
            && inner.names.get(&task.name) == Some(&id)
        {
            inner.names.remove(&task.name);
        }
        drop(inner);
        self.exited.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        #[allow(clippy::unwrap_used)]
        self.inner.lock().unwrap()
    }
}

/// A task's thread body: invoke one bounded pass forever, re-resolving
/// the body's deployed name each time, until a drain, a hard stop, or
/// the fault budget ends it.
fn task_main(
    runtime: &Arc<TaskRuntime>,
    name: &Arc<str>,
    body: &Value,
    signal: &Arc<DrainSignal>,
    current_pass: &Arc<Mutex<Option<blake3::Hash>>>,
) -> Ending {
    // Read the epoch *before* building: a load that races the build is
    // caught by the next pass's staleness check instead of lost.
    let mut synced_epoch = runtime.core.epoch();
    let mut vm = runtime.core.build_vm();
    install_drain_natives(&mut vm, &runtime.network, signal);
    // Tasks can declare and drain other tasks (never as deploy
    // declarations — reconciliation belongs to the entry).
    install_task_natives(&mut vm, runtime, false);

    let mut consecutive_faults: u32 = 0;
    loop {
        // A request that lands between passes: nothing to unwind, the
        // task just stops looping.
        if signal.is_requested() {
            return Ending::Drained { cleanly: true };
        }

        // Top up this VM with any generation deployed since the last
        // pass, so the resolution below always has its target loaded.
        let epoch = runtime.core.epoch();
        if epoch != synced_epoch {
            runtime.core.load_into(&mut vm);
            synced_epoch = epoch;
        }

        // The Live::latest resolution, applied by the runtime: the
        // pass runs whatever the body's deployed name currently binds.
        // (A closure resolves to itself — pinned, by design.)
        let result = match body {
            Value::FunctionRef(hash) => {
                let latest = runtime.core.latest(hash);
                // Publish the pass's resolution for the retirement
                // trace: while this pass runs, its frames hold code
                // reachable from exactly this hash (plus values that
                // are themselves rooted).
                if let Ok(mut current) = current_pass.lock() {
                    *current = Some(latest);
                }
                vm.call(&latest, Vec::new())
            }
            Value::Closure(c) => {
                vm.call_closure(&c.function_hash, Vec::new(), c.environment.clone())
            }
            // require_task_body pinned the shape at ensure time.
            other => {
                let error = format!("task body is not callable: {}", other.type_name());
                (runtime.events)(&TaskEvent::Faulted {
                    name: Arc::clone(name),
                    error,
                    restarting: false,
                });
                return Ending::Parked;
            }
        };

        match result {
            Ok(_) => {
                consecutive_faults = 0;
                // A pass that completed while a drain was pending: the
                // Drain::requested arm ran (or the pass never hit an
                // interruptible perform again) — that *is* the clean
                // wind-down.
                if signal.is_requested() {
                    return Ending::Drained { cleanly: true };
                }
            }
            // The unwind surfaced with no Drain::requested arm in
            // scope: drained, but without cleanup.
            Err(VmError::UnhandledAbility { ability_id, method })
                if ability_id == ambient_core::drain::ability_id()
                    && method == ambient_core::drain::requested_method_key() =>
            {
                return Ending::Drained { cleanly: false };
            }
            // The drain deadline's backstop. Never restart: the VM's
            // interrupt flag stays set, so another pass would only
            // hard-stop again.
            Err(VmError::HardStopped) => {
                (runtime.events)(&TaskEvent::Faulted {
                    name: Arc::clone(name),
                    error: "hard-stopped at the drain deadline".to_string(),
                    restarting: false,
                });
                return Ending::Parked;
            }
            Err(e) => {
                let error = vm.runtime_error(e).to_string();
                // Mid-drain faults (a cleanup arm that itself faults)
                // end the task rather than restarting it into a signal
                // that unwinds every interruptible perform.
                if signal.is_requested() {
                    (runtime.events)(&TaskEvent::Faulted {
                        name: Arc::clone(name),
                        error,
                        restarting: false,
                    });
                    return Ending::Drained { cleanly: false };
                }
                consecutive_faults += 1;
                let parked = consecutive_faults >= MAX_CONSECUTIVE_FAULTS;
                (runtime.events)(&TaskEvent::Faulted {
                    name: Arc::clone(name),
                    error,
                    restarting: !parked,
                });
                if parked {
                    return Ending::Parked;
                }
            }
        }
    }
}

/// Check that a task body is a zero-parameter function in some deployed
/// generation (the `Process::spawn` arity precedent).
fn require_task_body(core: &Arc<DeployRuntime>, body: &Value) -> Result<(), VmError> {
    let hash = match body {
        Value::Closure(c) => c.function_hash,
        Value::FunctionRef(h) => *h,
        other => {
            return Err(VmError::exception(format!(
                "Task.ensure: body must be a function, got {}",
                other.type_name()
            )));
        }
    };
    let Some(func) = core.lookup_function(&hash) else {
        return Err(VmError::exception(
            "Task.ensure: body references unknown code (hash not deployed)",
        ));
    };
    if func.param_count != 0 {
        return Err(VmError::exception(format!(
            "Task.ensure: body must take no parameters, takes {}",
            func.param_count
        )));
    }
    Ok(())
}

/// Install the `task_*` native implementations on a VM. `deploy` marks
/// the reconciliation entry's VM: its ensures are declarations the
/// reconciler diffs against the live registry (and its tasks are roots,
/// drained when a later pass stops declaring them). Uuid-keyed, so
/// these overwrite the not-wired stubs — per-VM wiring, exactly like
/// the process runtime's `process_*` natives.
pub fn install_task_natives(vm: &mut Vm, runtime: &Arc<TaskRuntime>, deploy: bool) {
    // task_ensure(name, body) -> ()
    let rt = Arc::clone(runtime);
    vm.register_native_impl(
        native_uuid("task_ensure"),
        Arc::new(move |args: Vec<Value>| {
            let name = crate::extract_string(&args)?;
            let body = args
                .get(1)
                .cloned()
                .ok_or_else(|| VmError::exception("Task.ensure: missing body argument"))?;
            rt.ensure(deploy, &name, body)?;
            Ok(Value::Unit)
        }),
    );

    // task_drain(name) -> ()
    let rt = Arc::clone(runtime);
    vm.register_native_impl(
        native_uuid("task_drain"),
        Arc::new(move |args: Vec<Value>| {
            rt.drain(&crate::extract_string(&args)?);
            Ok(Value::Unit)
        }),
    );
}
