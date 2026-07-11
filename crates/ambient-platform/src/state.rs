//! The runtime cell table behind the `State` ability (see
//! ref/live-upgrade.md, "State cells").
//!
//! One [`StateCells`] is owned by the deploy runtime and shared by every
//! VM it builds — entry/reconcile VMs, process VMs, future task VMs — so
//! a cell written in one generation is read by the next with no handoff:
//! the runtime owns the state, and cells belong to no generation. This is
//! the shared-handle-table pattern (`NetworkState` is the precedent), but
//! the table lives in the deploy core rather than the CLI host, because
//! deploy validation (Phase 4's migration fingerprints) needs to see
//! cells *before* the name swap.
//!
//! Concurrency contract:
//!
//! - `get`/`set` take the cell's lock briefly; `update` holds it across
//!   the user-supplied `f`, which runs reentrantly on the calling VM via
//!   [`Vm::invoke`] — that is the atomicity.
//! - Reentrancy is a fault, not a deadlock: each cell records the thread
//!   currently updating it, and any `State` access to the same cell from
//!   inside its own `f` raises a catchable exception naming the cell.
//! - Touching *other* cells from inside `f` is legal but can deadlock
//!   against a concurrent update locking in the opposite order — the
//!   ability docs tell authors to keep `f` a pure transformation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::ThreadId;

use ambient_ability::{Value, VmError};
use ambient_engine::vm::Vm;

/// One named cell: its value, and the thread currently holding it for an
/// `update` (the reentrancy detector).
struct Cell {
    value: Mutex<Value>,
    /// `Some(thread)` exactly while that thread holds `value`'s lock
    /// inside [`StateCells::update`].
    updating: Mutex<Option<ThreadId>>,
}

impl Cell {
    fn new(value: Value) -> Arc<Self> {
        Arc::new(Self {
            value: Mutex::new(value),
            updating: Mutex::new(None),
        })
    }

    /// Fault if the calling thread is inside its own `update` of this
    /// cell — the value lock is held, so proceeding would deadlock.
    fn check_reentrancy(&self, name: &str, op: &str) -> Result<(), VmError> {
        if *lock(&self.updating)? == Some(std::thread::current().id()) {
            return Err(VmError::exception(format!(
                "State.{op}: reentrant access to cell `{name}` from inside its own State.update"
            )));
        }
        Ok(())
    }
}

/// Thread-safe named cell table shared across all VMs and generations.
#[derive(Default)]
pub struct StateCells {
    /// Cell lookup, behind a short-lived lock — never held while a cell's
    /// own value lock is taken or user code runs.
    cells: Mutex<HashMap<Arc<str>, Arc<Cell>>>,
}

/// Lock a mutex, translating poisoning (a panicking thread mid-access)
/// into a loud fault instead of unwrapping.
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, VmError> {
    mutex
        .lock()
        .map_err(|_| VmError::exception("state cell lock poisoned"))
}

impl StateCells {
    /// An empty cell table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn cell(&self, name: &str) -> Result<Option<Arc<Cell>>, VmError> {
        Ok(lock(&self.cells)?.get(name).cloned())
    }

    fn existing(&self, name: &str, op: &str) -> Result<Arc<Cell>, VmError> {
        self.cell(name)?.ok_or_else(|| {
            VmError::exception(format!(
                "State.{op}: no cell named `{name}` (State.init creates cells)"
            ))
        })
    }

    /// `State::init`: adopt the cell if it exists, else create it with
    /// `make()`, invoked reentrantly on `vm`.
    ///
    /// The table lock is *not* held while `make` runs (user code must
    /// never run under it — it may init other cells). Under a concurrent
    /// init of the same cell the first insertion wins and the loser's
    /// value is discarded, so `make` should stay a pure constructor.
    ///
    /// # Errors
    ///
    /// Faults if `make` is not a function or its invocation faults.
    pub fn init(&self, vm: &mut Vm, name: &str, make: &Value) -> Result<(), VmError> {
        require_function(make, "init", "make")?;
        if self.cell(name)?.is_some() {
            return Ok(()); // Adopt: the every-deploy no-op.
        }
        let value = vm.invoke(make, Vec::new())?;
        lock(&self.cells)?
            .entry(Arc::from(name))
            .or_insert_with(|| Cell::new(value));
        Ok(())
    }

    /// `State::get`: the cell's current value.
    ///
    /// # Errors
    ///
    /// Faults if no such cell exists, or on reentrant access from inside
    /// the same cell's `update`.
    pub fn get(&self, name: &str) -> Result<Value, VmError> {
        let cell = self.existing(name, "get")?;
        cell.check_reentrancy(name, "get")?;
        Ok(lock(&cell.value)?.clone())
    }

    /// `State::set`: overwrite the cell's value.
    ///
    /// # Errors
    ///
    /// Faults if no such cell exists, or on reentrant access from inside
    /// the same cell's `update`.
    pub fn set(&self, name: &str, value: Value) -> Result<(), VmError> {
        let cell = self.existing(name, "set")?;
        cell.check_reentrancy(name, "set")?;
        *lock(&cell.value)? = value;
        Ok(())
    }

    /// `State::update`: atomic read-modify-write. Holds the cell's lock
    /// across `f` (invoked reentrantly on `vm`), stores the result, and
    /// returns it. On any fault in `f` the cell keeps its previous value.
    ///
    /// # Errors
    ///
    /// Faults if no such cell exists, `f` is not a function, `f`'s
    /// invocation faults, or on reentrant access to the same cell.
    pub fn update(&self, vm: &mut Vm, name: &str, f: &Value) -> Result<Value, VmError> {
        require_function(f, "update", "f")?;
        let cell = self.existing(name, "update")?;
        cell.check_reentrancy(name, "update")?;

        let mut value = lock(&cell.value)?;
        *lock(&cell.updating)? = Some(std::thread::current().id());
        let result = vm.invoke(f, vec![value.clone()]);
        *lock(&cell.updating)? = None;

        let new = result?;
        *value = new.clone();
        Ok(new)
    }
}

/// The `make`/`f` parameters are bare generics (ability signatures cannot
/// yet express effect-polymorphic function parameters), so the runtime
/// enforces the function-shaped contract, like `Live::latest`.
fn require_function(value: &Value, op: &str, role: &str) -> Result<(), VmError> {
    match value {
        Value::FunctionRef(_) | Value::Closure(_) => Ok(()),
        other => Err(VmError::exception(format!(
            "State.{op}: {role} must be a function, got {}",
            other.type_name()
        ))),
    }
}

/// Extract the `(name, second)` argument pair shared by every binary
/// state native.
fn name_and(mut args: Vec<Value>) -> Result<(String, Value), VmError> {
    let second = args.pop();
    let name = crate::extract_string(&args)?;
    let second = second.ok_or_else(|| VmError::TypeErrorOwned {
        expected: "two arguments".to_string(),
        got: "one".to_string(),
    })?;
    Ok((name, second))
}

/// Install the real `State` natives on a VM, all capturing one shared
/// cell table. `state_get`/`state_set` are pure lookups; `state_init` and
/// `state_update` are VM-invoking (they run user functions), so they
/// register through the per-VM channel — uuid-keyed, overriding the
/// not-wired stubs, exactly like `live_latest`'s real resolution.
pub(crate) fn register_state_natives(vm: &mut Vm, cells: &Arc<StateCells>) {
    let table = Arc::clone(cells);
    vm.register_native_vm_impl(
        crate::native_uuid("state_init"),
        Arc::new(move |vm, args| {
            let (name, make) = name_and(args)?;
            table.init(vm, &name, &make)?;
            Ok(Value::Unit)
        }),
    );

    let table = Arc::clone(cells);
    vm.register_native_impl(
        crate::native_uuid("state_get"),
        Arc::new(move |args| table.get(&crate::extract_string(&args)?)),
    );

    let table = Arc::clone(cells);
    vm.register_native_impl(
        crate::native_uuid("state_set"),
        Arc::new(move |args| {
            let (name, value) = name_and(args)?;
            table.set(&name, value)?;
            Ok(Value::Unit)
        }),
    );

    let table = Arc::clone(cells);
    vm.register_native_vm_impl(
        crate::native_uuid("state_update"),
        Arc::new(move |vm, args| {
            let (name, f) = name_and(args)?;
            table.update(vm, &name, &f)
        }),
    );
}
