//! The runtime cell table behind the `State` ability (see
//! ref/live-upgrade.md, "State cells").
//!
//! One [`StateCells`] is owned by the deploy runtime and shared by every
//! VM it builds — entry/reconcile VMs and task VMs alike — so
//! a cell written in one generation is read by the next with no handoff:
//! the runtime owns the state, and cells belong to no generation. This is
//! the shared-handle-table pattern (`NetworkState` is the precedent), but
//! the table lives in the deploy core rather than the CLI host, because
//! deploy validation (the migration fingerprints) needs to see cells
//! *before* the name swap.
//!
//! Every cell records the **fingerprint** of the static type it was last
//! written at — the canonical rendering the compiler threads through each
//! write perform (see `ref/live-upgrade.md`, "Migration"). `init` create,
//! `set`, and `update` commit all stamp it; `init_versioned` dispatches
//! on it: no cell → `make`, fingerprint `new` → adopt, fingerprint `old`
//! → `migrate` (fingerprint advances), anything else → fault. Deploy
//! validation checks statically-named `init_versioned` sites against this
//! table pre-swap, so the fault path is reserved for computed cell names
//! and races.
//!
//! Concurrency contract:
//!
//! - `get`/`set` take the cell's lock briefly; `update` (and
//!   `init_versioned`'s migrate arm) hold it across the user-supplied
//!   function, which runs reentrantly on the calling VM via
//!   [`Vm::invoke`] — that is the atomicity.
//! - Reentrancy is a fault, not a deadlock: each cell records the thread
//!   currently updating it, and any `State` access to the same cell from
//!   inside its own `f`/`migrate` raises a catchable exception naming the
//!   cell.
//! - Touching *other* cells from inside `f` is legal but can deadlock
//!   against a concurrent update locking in the opposite order — the
//!   ability docs tell authors to keep `f` a pure transformation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::ThreadId;

use ambient_ability::{Value, VmError};
use ambient_engine::vm::Vm;

/// A cell's value together with the fingerprint of the static type it was
/// last written at. They change together, under one lock.
struct CellContent {
    value: Value,
    fingerprint: Arc<str>,
}

/// One named cell: its content, and the thread currently holding it for
/// an `update`/`migrate` (the reentrancy detector).
struct Cell {
    content: Mutex<CellContent>,
    /// `Some(thread)` exactly while that thread holds `content`'s lock
    /// across user code ([`StateCells::update`] or the migrate arm of
    /// [`StateCells::init_versioned`]).
    updating: Mutex<Option<ThreadId>>,
}

impl Cell {
    fn new(value: Value, fingerprint: &str) -> Arc<Self> {
        Arc::new(Self {
            content: Mutex::new(CellContent {
                value,
                fingerprint: Arc::from(fingerprint),
            }),
            updating: Mutex::new(None),
        })
    }

    /// Fault if the calling thread is inside its own `update` of this
    /// cell — the content lock is held, so proceeding would deadlock.
    fn check_reentrancy(&self, name: &str, op: &str) -> Result<(), VmError> {
        if *lock(&self.updating)? == Some(std::thread::current().id()) {
            return Err(VmError::exception(format!(
                "State.{op}: reentrant access to cell `{name}` from inside its own State.update"
            )));
        }
        Ok(())
    }

    /// Run `f` on the current value (invoked reentrantly on `vm`) while
    /// holding `content`'s lock, with the reentrancy stamp set; commit
    /// the result and `fingerprint` only on success. The caller passes
    /// the guard so a fingerprint check and the invocation are one
    /// atomic step.
    fn invoke_and_commit(
        &self,
        content: &mut CellContent,
        vm: &mut Vm,
        f: &Value,
        fingerprint: &str,
    ) -> Result<Value, VmError> {
        *lock(&self.updating)? = Some(std::thread::current().id());
        let result = vm.invoke(f, vec![content.value.clone()]);
        *lock(&self.updating)? = None;

        let new = result?;
        content.value = new.clone();
        content.fingerprint = Arc::from(fingerprint);
        Ok(new)
    }
}

/// Thread-safe named cell table shared across all VMs and generations.
#[derive(Default)]
pub struct StateCells {
    /// Cell lookup, behind a short-lived lock — never held while a cell's
    /// own content lock is taken or user code runs.
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

    /// Snapshot every cell's current value — the cell half of the
    /// retirement trace's roots (`ref/live-upgrade.md`, "Retirement").
    /// Takes each cell's content lock briefly, one at a time, after
    /// releasing the table lock; a cell mid-`update` is waited on (its
    /// committed value is what the trace should see). Poisoned cells are
    /// skipped — their thread panicked, and a trace is not the place to
    /// fault.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(Arc<str>, Value)> {
        // The table guard must drop before any content lock is taken:
        // an `update` holds its cell's content lock and may take the
        // table lock for nested cell access — holding both here would
        // be an ABBA deadlock.
        let entries: Vec<(Arc<str>, Arc<Cell>)> = match lock(&self.cells) {
            Ok(cells) => cells
                .iter()
                .map(|(name, cell)| (Arc::clone(name), Arc::clone(cell)))
                .collect(),
            Err(_) => return Vec::new(),
        };
        entries
            .into_iter()
            .filter_map(|(name, cell)| {
                let content = lock(&cell.content).ok()?;
                Some((name, content.value.clone()))
            })
            .collect()
    }

    fn existing(&self, name: &str, op: &str) -> Result<Arc<Cell>, VmError> {
        self.cell(name)?.ok_or_else(|| {
            VmError::exception(format!(
                "State.{op}: no cell named `{name}` (State.init creates cells)"
            ))
        })
    }

    /// Create the cell with `make()` (invoked reentrantly on `vm`),
    /// stamped with `fingerprint`. The table lock is *not* held while
    /// `make` runs (user code must never run under it — it may init other
    /// cells). Under a concurrent creation of the same cell the first
    /// insertion wins and the loser's value is discarded, so `make`
    /// should stay a pure constructor.
    fn create(
        &self,
        vm: &mut Vm,
        name: &str,
        make: &Value,
        fingerprint: &str,
    ) -> Result<(), VmError> {
        let value = vm.invoke(make, Vec::new())?;
        lock(&self.cells)?
            .entry(Arc::from(name))
            .or_insert_with(|| Cell::new(value, fingerprint));
        Ok(())
    }

    /// `State::init`: adopt the cell if it exists, else create it with
    /// `make()`, stamped with the writer's fingerprint.
    ///
    /// # Errors
    ///
    /// Faults if `make` is not a function or its invocation faults.
    pub fn init(
        &self,
        vm: &mut Vm,
        name: &str,
        make: &Value,
        fingerprint: &str,
    ) -> Result<(), VmError> {
        require_function(make, "init", "make")?;
        if self.cell(name)?.is_some() {
            return Ok(()); // Adopt: the every-deploy no-op.
        }
        self.create(vm, name, make, fingerprint)
    }

    /// `State::init_versioned`: adopt, migrate, or create, dispatching on
    /// the cell's recorded fingerprint (see the module docs). `migrate`
    /// runs under the cell's content lock, like `update`, so concurrent
    /// readers never observe a half-migrated cell.
    ///
    /// # Errors
    ///
    /// Faults if `make`/`migrate` is not a function, if their invocation
    /// faults, on reentrant access, or if the cell's fingerprint matches
    /// neither `old_fingerprint` nor `new_fingerprint` — the arm deploy
    /// validation rejects pre-swap when the cell name is static.
    pub fn init_versioned(
        &self,
        vm: &mut Vm,
        name: &str,
        make: &Value,
        migrate: &Value,
        old_fingerprint: &str,
        new_fingerprint: &str,
    ) -> Result<(), VmError> {
        require_function(make, "init_versioned", "make")?;
        require_function(migrate, "init_versioned", "migrate")?;
        let Some(cell) = self.cell(name)? else {
            return self.create(vm, name, make, new_fingerprint);
        };
        cell.check_reentrancy(name, "init_versioned")?;

        // Dispatch under the content lock so the fingerprint check and
        // the migration are one atomic step against concurrent writers.
        let mut content = lock(&cell.content)?;
        if content.fingerprint.as_ref() == new_fingerprint {
            return Ok(()); // Adopt: the every-deploy no-op.
        }
        if content.fingerprint.as_ref() == old_fingerprint {
            cell.invoke_and_commit(&mut content, vm, migrate, new_fingerprint)?;
            return Ok(());
        }
        let current = Arc::clone(&content.fingerprint);
        Err(VmError::exception(format!(
            "State.init_versioned: cell `{name}` is at type `{current}`, which is neither \
             the migration's old type `{old_fingerprint}` nor its new type `{new_fingerprint}`"
        )))
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
        Ok(lock(&cell.content)?.value.clone())
    }

    /// The fingerprint a cell was last written at, if the cell exists —
    /// deploy validation's pre-swap view of the migration table.
    ///
    /// # Errors
    ///
    /// Faults only on a poisoned lock.
    pub fn fingerprint(&self, name: &str) -> Result<Option<Arc<str>>, VmError> {
        match self.cell(name)? {
            Some(cell) => Ok(Some(lock(&cell.content)?.fingerprint.clone())),
            None => Ok(None),
        }
    }

    /// `State::set`: overwrite the cell's value, stamping the writer's
    /// fingerprint.
    ///
    /// # Errors
    ///
    /// Faults if no such cell exists, or on reentrant access from inside
    /// the same cell's `update`.
    pub fn set(&self, name: &str, value: Value, fingerprint: &str) -> Result<(), VmError> {
        let cell = self.existing(name, "set")?;
        cell.check_reentrancy(name, "set")?;
        let mut content = lock(&cell.content)?;
        content.value = value;
        content.fingerprint = Arc::from(fingerprint);
        Ok(())
    }

    /// `State::update`: atomic read-modify-write. Holds the cell's lock
    /// across `f` (invoked reentrantly on `vm`), stores the result with
    /// the writer's fingerprint, and returns it. On any fault in `f` the
    /// cell keeps its previous value and fingerprint.
    ///
    /// # Errors
    ///
    /// Faults if no such cell exists, `f` is not a function, `f`'s
    /// invocation faults, or on reentrant access to the same cell.
    pub fn update(
        &self,
        vm: &mut Vm,
        name: &str,
        f: &Value,
        fingerprint: &str,
    ) -> Result<Value, VmError> {
        require_function(f, "update", "f")?;
        let cell = self.existing(name, "update")?;
        cell.check_reentrancy(name, "update")?;
        let mut content = lock(&cell.content)?;
        cell.invoke_and_commit(&mut content, vm, f, fingerprint)
    }
}

/// Backstop for dynamic paths: the checker now enforces the
/// `make`/`migrate`/`f` function shapes for ordinary `State` perform
/// sites (they are real effect-polymorphic function types in the ability
/// signature), but values can still reach the native from hand-written
/// handlers or remote/dynamic invocation, so the runtime re-checks that
/// each is a function, like `Live::latest`.
fn require_function(value: &Value, op: &str, role: &str) -> Result<(), VmError> {
    match value {
        Value::FunctionRef(_) | Value::Closure(_) => Ok(()),
        other => Err(VmError::exception(format!(
            "State.{op}: {role} must be a function, got {}",
            other.type_name()
        ))),
    }
}

/// Extract exactly `N` arguments, with the leading one a cell name.
fn name_and<const N: usize>(args: Vec<Value>) -> Result<(String, [Value; N]), VmError> {
    let name = crate::extract_string(&args)?;
    let rest: [Value; N] = args
        .into_iter()
        .skip(1)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| VmError::TypeErrorOwned {
            expected: format!("{} arguments", N + 1),
            got: "fewer".to_string(),
        })?;
    Ok((name, rest))
}

/// A hidden fingerprint argument, threaded by the compiler as a string.
fn fingerprint_arg(value: &Value, op: &str) -> Result<String, VmError> {
    match value {
        Value::String(s) => Ok(s.to_string()),
        other => Err(VmError::TypeErrorOwned {
            expected: format!("a fingerprint string for State.{op}"),
            got: other.type_name().to_string(),
        }),
    }
}

/// Install the real `State` natives on a VM, all capturing one shared
/// cell table. `state_get` is a pure lookup and `state_set` a pure store;
/// `state_init`, `state_update`, and `state_init_versioned` are
/// VM-invoking (they run user functions), so they register through the
/// per-VM channel — uuid-keyed, overriding the not-wired stubs, exactly
/// like `live_latest`'s real resolution.
pub(crate) fn register_state_natives(vm: &mut Vm, cells: &Arc<StateCells>) {
    let table = Arc::clone(cells);
    vm.register_native_vm_impl(
        crate::native_uuid("state_init"),
        Arc::new(move |vm, args| {
            let (name, [make, fingerprint]) = name_and::<2>(args)?;
            let fingerprint = fingerprint_arg(&fingerprint, "init")?;
            table.init(vm, &name, &make, &fingerprint)?;
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
            let (name, [value, fingerprint]) = name_and::<2>(args)?;
            let fingerprint = fingerprint_arg(&fingerprint, "set")?;
            table.set(&name, value, &fingerprint)?;
            Ok(Value::Unit)
        }),
    );

    let table = Arc::clone(cells);
    vm.register_native_vm_impl(
        crate::native_uuid("state_update"),
        Arc::new(move |vm, args| {
            let (name, [f, fingerprint]) = name_and::<2>(args)?;
            let fingerprint = fingerprint_arg(&fingerprint, "update")?;
            table.update(vm, &name, &f, &fingerprint)
        }),
    );

    let table = Arc::clone(cells);
    vm.register_native_vm_impl(
        crate::native_uuid("state_init_versioned"),
        Arc::new(move |vm, args| {
            let (name, [make, migrate, old, new]) = name_and::<4>(args)?;
            let old = fingerprint_arg(&old, "init_versioned")?;
            let new = fingerprint_arg(&new, "init_versioned")?;
            table.init_versioned(vm, &name, &make, &migrate, &old, &new)?;
            Ok(Value::Unit)
        }),
    );
}
