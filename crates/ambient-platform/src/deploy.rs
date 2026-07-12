//! The deploy core: generations, the runtime name table, and the deploy
//! operation (see `ref/live-upgrade.md`).
//!
//! A [`Generation`] is one deployable snapshot of a program: its canonical
//! content-addressed objects plus **name bindings** — `Fqn → (hash,
//! canonical signature)`, the same names the disk store and `.ambient`
//! packs record. A [`DeployRuntime`] owns everything a deploy touches:
//!
//! 1. **Load** the generation's objects additively — old hashes stay
//!    resident forever, so generations coexist collision-free.
//! 2. **Validate** the generation against the loaded view. Failures reject
//!    the deploy here, with the previous name table untouched.
//! 3. **Swap** the runtime-wide name table atomically. Readers hold an
//!    immutable snapshot (`Arc`), so a resolution sees exactly one
//!    generation's bindings — never a torn mixture. The swap applies the
//!    **rebinding rule**: a name whose canonical signature changed (or is
//!    missing on either side) is retire-and-fresh, never a rebinding —
//!    `Live::latest` stops following the retired hashes, so old refs
//!    resolve to themselves. Alongside the table lives its reverse map
//!    (hash → deployed names), which is how `Live::latest!` maps a
//!    compile-time ref to the current binding of the name it was deployed
//!    under.
//! 4. **Reconcile**: run the new entry function on a freshly built VM. The
//!    caller wires the VM first (a hook), which is how a client like the
//!    task runtime installs its own natives without this module knowing
//!    what a task is.
//! 5. **Report** the exact hash diff: names rebound, added, unchanged.
//!
//! This module is client-agnostic. Every frontend (dev loop, REPL,
//! remote deploy) applies generations through the same operation.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use ambient_ability::{Value, VmError};
use ambient_engine::bytecode::CompiledFunction;
use ambient_engine::compiler::{CompiledModule, MigrationRecord};
use ambient_engine::vm::Vm;

/// Builds a base VM: platform natives registered (Stdio, Network, ...), no
/// code loaded. The deploy runtime layers every loaded generation on top;
/// clients layer their own natives via the deploy wire hook.
pub type VmFactory = Arc<dyn Fn() -> Vm + Send + Sync>;

/// One name's binding: the content hash it resolves to, plus the canonical
/// type signature it was checked at (the rebinding rule's input — a name
/// whose signature changed is retire-and-fresh, never a rebinding).
///
/// The signature is `None` when the producing pipeline didn't render one
/// (hand-assembled generations in tests). `None` never compares equal —
/// not even to another `None` — so missing data always classifies as
/// retire-and-fresh, never as a silent rebinding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Content hash of the bound object (function or const value).
    pub hash: blake3::Hash,
    /// Canonical type signature rendering (see the engine's
    /// `CheckResult::signatures`).
    pub signature: Option<Arc<str>>,
}

/// A code generation: every compiled function of a build, the
/// content-addressed `const` value and native objects those functions
/// reference, and the name bindings the build declared.
#[derive(Default)]
pub struct Generation {
    /// Function hash → runnable function.
    pub functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,
    /// Value-object hash → the `const` value it holds.
    pub values: HashMap<blake3::Hash, Value>,
    /// Native-object hash → its `(uuid, param_count)` identity. Loaded so
    /// calls to an extern fn dispatch to the host implementation registered
    /// on the VM.
    pub natives: HashMap<blake3::Hash, (uuid::Uuid, u8)>,
    /// Name bindings: fully-qualified item name → (hash, signature).
    /// Dispatch symbols (`<uuid>::method`) are content-addressed, never
    /// late-bound, so they are deliberately absent.
    pub bindings: HashMap<Arc<str>, Binding>,
    /// Statically-named `State::init_versioned` obligations the build
    /// declared (see `ref/live-upgrade.md`, "Migration"). Validation
    /// checks each against the live cell table *before* the name swap:
    /// a cell whose fingerprint matches neither side rejects the deploy
    /// with the previous generation untouched.
    pub migrations: Vec<MigrationRecord>,
}

/// A shared code generation.
pub type Functions = Arc<Generation>;

/// Share a compiled module as a code generation: its functions, `const`
/// value objects, native objects, and name bindings.
#[must_use]
pub fn functions_from_module(compiled: &CompiledModule) -> Functions {
    let functions = compiled
        .functions
        .iter()
        .map(|(hash, func)| (*hash, Arc::new(func.clone())))
        .collect();
    // Value objects live in `objects`; pull out each one's `const` value.
    let values = compiled
        .objects
        .iter()
        .filter_map(|(hash, object)| Some((*hash, object.as_value()?)))
        .collect();
    // Native objects too: each carries an extern fn's (uuid, arity).
    let natives = compiled
        .objects
        .iter()
        .filter_map(|(hash, object)| Some((*hash, object.as_native()?)))
        .collect();
    // Named items only: dispatch symbols are content identities, not names.
    let bindings = compiled
        .function_names
        .iter()
        .chain(&compiled.const_names)
        .filter(|(name, _)| !is_dispatch_symbol(name))
        .map(|(name, hash)| {
            let binding = Binding {
                hash: *hash,
                signature: compiled.signatures.get(name).cloned(),
            };
            (Arc::clone(name), binding)
        })
        .collect();
    Arc::new(Generation {
        functions,
        values,
        natives,
        bindings,
        migrations: compiled.migrations.clone(),
    })
}

/// Whether a store name is a content dispatch symbol (`<uuid>::method` for
/// ability default implementations and nominal-type methods) rather than a
/// module-qualified item name.
#[must_use]
pub fn is_dispatch_symbol(name: &str) -> bool {
    name.split("::")
        .next()
        .is_some_and(|head| uuid::Uuid::parse_str(head).is_ok())
}

/// The runtime-wide name table: one immutable snapshot per deploy.
pub type NameTable = HashMap<Arc<str>, Binding>;

/// One deploy's name-resolution snapshot: the atomic table plus its
/// reverse map. Both swap together, so a single read sees one
/// generation's view of both directions.
#[derive(Default)]
struct NameState {
    /// Fqn → current binding.
    table: Arc<NameTable>,
    /// Hash → every name it was deployed under and not retired from.
    /// `Live::latest` resolves through this: old ref's hash → deployed
    /// name → that name's *current* hash. Retire-and-fresh removes the
    /// name's old hashes, so refs into a retired lineage resolve to
    /// themselves.
    reverse: HashMap<blake3::Hash, BTreeSet<Arc<str>>>,
}

/// Shared handle to the name state. Separately `Arc`ed so per-VM natives
/// (the `live_latest` implementation) can capture it and outlive any one
/// borrow of the [`DeployRuntime`].
#[derive(Default)]
struct NameResolver {
    state: RwLock<Arc<NameState>>,
}

impl NameResolver {
    /// An immutable snapshot: two lookups against one snapshot can never
    /// straddle a deploy.
    fn snapshot(&self) -> Arc<NameState> {
        #[allow(clippy::unwrap_used)]
        Arc::clone(&self.state.read().unwrap())
    }

    /// One `latest` read: the current hash of the name `hash` was deployed
    /// under. Identity when the hash was never deployed under a name, when
    /// its names left the table, or when it was deployed under several
    /// names whose bindings have since diverged (no single answer exists —
    /// resolving to itself is the only consistent choice).
    fn latest(&self, hash: blake3::Hash) -> blake3::Hash {
        let state = self.snapshot();
        let Some(names) = state.reverse.get(&hash) else {
            return hash;
        };
        let mut current = None;
        for name in names {
            let Some(binding) = state.table.get(name) else {
                continue;
            };
            match current {
                None => current = Some(binding.hash),
                Some(hash) if hash == binding.hash => {}
                Some(_) => return hash,
            }
        }
        current.unwrap_or(hash)
    }

    /// The `live_latest` native: resolve a function ref to the current
    /// binding of the name it was deployed under.
    fn latest_native(&self, args: Vec<Value>) -> Result<Value, VmError> {
        match function_arg(args)? {
            Value::FunctionRef(hash) => Ok(Value::FunctionRef(self.latest(hash))),
            // A closure is code fused to captured state; rebinding the code
            // half under a kept environment is deliberately excluded
            // (ref/live-upgrade.md), so it resolves to itself.
            other => Ok(other),
        }
    }
}

/// Extract `live_latest`'s single argument and check it is
/// function-shaped. `Live::latest` stays identity-typed over `F` (it is
/// applied at every arity and the language has no arity polymorphism), so
/// the checker cannot pin it to a function type — the runtime enforces the
/// function-shaped contract as a backstop for both static and dynamic
/// call paths.
fn function_arg(args: Vec<Value>) -> Result<Value, VmError> {
    let Some(value) = args.into_iter().next() else {
        return Err(VmError::exception("Live.latest: missing function argument"));
    };
    match &value {
        Value::FunctionRef(_) | Value::Closure(_) => Ok(value),
        other => Err(VmError::exception(format!(
            "Live.latest: expected a function, got {}",
            other.type_name()
        ))),
    }
}

/// The identity `live_latest` — the *stub* for the `live_latest` extern,
/// and the one deliberate deviation from "every stub raises not-wired":
/// under plain `ambient run` (no deploy runtime installing the real
/// resolution) `Live::latest!` must behave as identity so the program runs
/// identically, minus liveness (ref/live-upgrade.md). It still enforces
/// the function-shaped contract, so `run` and `dev` reject the same
/// arguments.
pub(crate) fn latest_identity(args: Vec<Value>) -> Result<Value, VmError> {
    function_arg(args)
}

/// The exact name diff of one deploy, computed hash-against-hash.
#[derive(Debug, Default)]
pub struct NameDiff {
    /// Names bound to a different hash with the same canonical signature,
    /// sorted. `Live::latest` follows these: the name's old hashes keep
    /// resolving forward to the new binding.
    pub rebound: Vec<Arc<str>>,
    /// Names whose canonical signature changed (or was missing on either
    /// side), sorted. Retire-and-fresh, never a rebinding: the old binding
    /// retired and the name entered the table fresh, so `latest` sites
    /// keep resolving old refs to themselves until their callers upgrade.
    pub retired: Vec<Arc<str>>,
    /// Names that entered the table fresh, sorted.
    pub added: Vec<Arc<str>>,
    /// Names bound to the identical hash (identical subtrees are skipped
    /// by construction).
    pub unchanged: usize,
}

/// Result of a successful deploy.
#[derive(Debug)]
pub struct DeployReport {
    /// The entry function's return value.
    pub value: Value,
    /// The name-table diff.
    pub names: NameDiff,
    /// The generation id this deploy was recorded as (1-based, one per
    /// successful swap) — the identity retirement tracing reports.
    pub generation: u64,
    /// Deploy diagnostics: changes the running system can never pick up
    /// and ability-method keys old live code performs uncovered (see
    /// `ref/live-upgrade.md`, "Deploy diagnostics").
    pub warnings: Vec<crate::retire::DeployWarning>,
}

/// The result of a dry-run [`DeployRuntime::plan`]: everything a deploy
/// computes *before* the swap, and nothing it does at or after it. The
/// generation's objects were loaded (additive and content-addressed, so a
/// planned-but-never-committed generation leaves only harmless residents),
/// but the name table, the retirement ledger, the cell table, and the
/// entry are all untouched.
///
/// A validation failure is a *successful* plan whose [`Self::problems`] is
/// non-empty, not an error: reporting what *would* be rejected — alongside
/// the diff it *would* produce — is the whole point of planning. (Contrast
/// [`DeployRuntime::deploy`], whose validation failure is a
/// [`DeployError::Validation`], because a deploy that cannot apply must not
/// silently succeed.)
#[derive(Debug, Default)]
pub struct PlanReport {
    /// The name diff this generation *would* produce against the current
    /// table, computed read-only through the exact per-name classification
    /// the swap uses ([`classify_name`]) — so a plan can never diverge from
    /// what a subsequent [`DeployRuntime::deploy`] then reports.
    pub names: NameDiff,
    /// Why the deploy *would* be rejected, if anything: an unloaded object
    /// behind a binding, an unloaded entry, or a migration fingerprint that
    /// matches neither side of the live cell table. Empty means the plan
    /// would apply cleanly.
    pub problems: Vec<String>,
}

/// One name's fate under a generation's bindings, against the current
/// table. The single classification both the read-only plan diff
/// ([`DeployRuntime::diff_bindings`]) and the mutating swap
/// ([`DeployRuntime::swap_names`]) key off, so the two can never drift.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameChange {
    /// Bound to the identical hash — same behavior, same type.
    Unchanged,
    /// A changed hash whose canonical signature is identical: a rebinding,
    /// `latest` follows it forward.
    Rebound,
    /// A changed hash whose signature changed (or is missing on either
    /// side): retire-and-fresh, `latest` resolves old refs to themselves.
    Retired,
    /// A name the table did not carry before.
    Added,
}

/// Classify one name's binding against its previous binding (if any) — the
/// rebinding rule, in one place. See [`NameChange`].
fn classify_name(prev: Option<&Binding>, next: &Binding) -> NameChange {
    match prev {
        Some(prev) if prev.hash == next.hash => NameChange::Unchanged,
        Some(prev) if same_signature(prev, next) => NameChange::Rebound,
        Some(_) => NameChange::Retired,
        None => NameChange::Added,
    }
}

/// Why a deploy failed.
#[derive(Debug)]
pub enum DeployError {
    /// The generation failed validation; the name table was not swapped
    /// and the previous generation is untouched. (Loaded objects stay
    /// resident — loading is additive and content-addressed, so a rejected
    /// generation's objects are inert.)
    Validation(Vec<String>),
    /// The entry function faulted during reconciliation. The name table
    /// *was* swapped (the new code is live for late-bound resolution);
    /// client-side reconciliation is incomplete.
    Entry(String),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(problems) => {
                write!(f, "deploy rejected: {}", problems.join("; "))
            }
            Self::Entry(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for DeployError {}

/// Objects loaded so far, cumulative across every deploy.
#[derive(Default)]
pub(crate) struct Loaded {
    pub(crate) functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,
    pub(crate) values: HashMap<blake3::Hash, Value>,
    pub(crate) natives: HashMap<blake3::Hash, (uuid::Uuid, u8)>,
}

/// The deploy core. One per running system; every frontend (dev loop,
/// REPL, remote) applies generations through [`Self::deploy`].
pub struct DeployRuntime {
    vm_factory: VmFactory,
    /// Cumulative object stores. Additive: a deploy only ever inserts.
    /// `Arc`ed so the per-VM `live_latest` native can capture it and top
    /// its VM up when a resolution outruns the VM's loaded set.
    loaded: Arc<Mutex<Loaded>>,
    /// Bumped whenever [`Self::load`] merges a generation — the cheap
    /// staleness check behind [`Self::epoch`], so a long-lived VM (a
    /// task's) can re-sync via [`Self::load_into`] only when something
    /// new arrived.
    epoch: std::sync::atomic::AtomicU64,
    /// The atomic name table plus its reverse map. Swapped wholesale per
    /// deploy; readers resolve against an immutable snapshot.
    names: Arc<NameResolver>,
    /// The `State` cell table: named, runtime-owned values that belong to
    /// no generation (see `ref/live-upgrade.md`, "State cells"). Owned
    /// here — not by the embedding host — so every VM this runtime builds
    /// shares it, and deploy validation can inspect cells pre-swap
    /// (Phase 4's migration fingerprints).
    cells: Arc<crate::state::StateCells>,
    /// The generation ledger behind retirement tracing (which deploy
    /// shipped which hashes, what names they carried, which generations
    /// have retired — see [`crate::retire`]).
    ledger: Mutex<crate::retire::Ledger>,
    /// Trace-root providers registered by registry clients (the task
    /// runtime). Cells are built in; everything else arrives through
    /// here.
    root_providers: Mutex<Vec<crate::retire::RootProvider>>,
}

impl DeployRuntime {
    /// Create a deploy runtime with nothing loaded, an empty name table,
    /// and an empty cell table.
    #[must_use]
    pub fn new(vm_factory: VmFactory) -> Self {
        Self {
            vm_factory,
            loaded: Arc::new(Mutex::new(Loaded::default())),
            epoch: std::sync::atomic::AtomicU64::new(0),
            names: Arc::new(NameResolver::default()),
            cells: Arc::new(crate::state::StateCells::new()),
            ledger: Mutex::new(crate::retire::Ledger::default()),
            root_providers: Mutex::new(Vec::new()),
        }
    }

    /// Register a trace-root provider (see [`crate::retire`]): a registry
    /// client's contribution to retirement tracing. Providers are called
    /// with no locks held, on every trace.
    ///
    /// # Panics
    ///
    /// Panics if the provider list's lock is poisoned.
    pub fn register_roots(&self, provider: crate::retire::RootProvider) {
        #[allow(clippy::unwrap_used)]
        self.root_providers.lock().unwrap().push(provider);
    }

    /// Run one retirement trace (see `ref/live-upgrade.md`, "Retirement"):
    /// gather roots (cells, registered providers, the current name
    /// table), walk everything they reach, and classify each old
    /// generation as retired (permanently) or pinned (with what pins it).
    ///
    /// # Panics
    ///
    /// Panics if a runtime lock is poisoned.
    #[must_use]
    pub fn retirement(&self) -> crate::retire::RetirementReport {
        let mut roots = self.runtime_roots();
        // Every currently bound name is reachable through late-bound
        // resolution at any time. (For a full build these all attribute
        // to the current generation; a partial generation leaves old
        // bindings standing, and those correctly keep their shipper
        // live.)
        for (name, binding) in self.name_table().iter() {
            roots.push(crate::retire::Root::from_hash(
                crate::retire::RootOrigin::Name(Arc::clone(name)),
                binding.hash,
            ));
        }

        let (origins, seeds) = root_seeds(&roots);
        let reach = {
            let loaded = self.lock_loaded();
            crate::retire::reach(&loaded, &seeds)
        };
        #[allow(clippy::unwrap_used)]
        self.ledger.lock().unwrap().classify(&reach, &origins)
    }

    /// The runtime-held trace roots: registered providers (the task
    /// registry) plus the cell table. Gathered while holding no
    /// runtime-wide locks — providers lock their own registries, cells
    /// lock the cell table.
    fn runtime_roots(&self) -> Vec<crate::retire::Root> {
        let providers: Vec<crate::retire::RootProvider> = {
            #[allow(clippy::unwrap_used)]
            self.root_providers.lock().unwrap().clone()
        };
        let mut roots: Vec<crate::retire::Root> = Vec::new();
        for provider in providers {
            roots.extend(provider());
        }
        for (name, value) in self.cells.snapshot() {
            roots.push(crate::retire::Root {
                origin: crate::retire::RootOrigin::Cell(name),
                value,
            });
        }
        roots
    }

    /// Compute the deploy warnings (see `ref/live-upgrade.md`, "Deploy
    /// diagnostics"), after reconciliation: `live` is what the running
    /// system holds (cells + registries — the just-ensured tasks
    /// included); `fresh` is what the live re-entry points reach — the
    /// entry, plus the forward resolution of every task root (the
    /// runtime re-resolves task bodies each pass; a cell value gets no
    /// such forwarding, which is exactly what makes a pinned holder's
    /// change unreachable).
    fn deploy_warnings(
        &self,
        entry: &blake3::Hash,
        changed: &[crate::retire::ChangedName],
    ) -> Vec<crate::retire::DeployWarning> {
        let roots = self.runtime_roots();
        let (origins, seeds) = root_seeds(&roots);

        let mut entries = vec![*entry];
        for (index, hashes) in &seeds {
            if matches!(origins[*index], crate::retire::RootOrigin::Task(_)) {
                entries.extend(hashes.iter().map(|hash| self.names.latest(*hash)));
            }
        }

        let loaded = self.lock_loaded();
        let live = crate::retire::reach(&loaded, &seeds);
        let fresh = crate::retire::reach(&loaded, &[(0, entries)]);
        #[allow(clippy::unwrap_used)]
        let ledger = self.ledger.lock().unwrap();
        ledger.warnings(&loaded, &live, &fresh, &origins, changed, &|hash| {
            self.names.latest(*hash)
        })
    }

    /// The shared `State` cell table (for inspection and, later,
    /// pre-swap migration validation).
    #[must_use]
    pub fn cells(&self) -> &Arc<crate::state::StateCells> {
        &self.cells
    }

    /// Dry-run a generation: everything [`Self::deploy`] does *before* the
    /// swap, and nothing at or after it. Loads the objects additively
    /// (inert — a planned generation's objects are harmless residents),
    /// runs the same validation (loaded-object checks plus the pre-swap
    /// migration-fingerprint checks against the live cell table), computes
    /// the *would-be* name diff read-only, and returns a [`PlanReport`].
    ///
    /// Does **not** swap the name table, record a generation in the
    /// retirement ledger, or run the entry — so a validation failure is a
    /// successful plan whose [`PlanReport::problems`] is non-empty, never an
    /// error (that is the whole point of planning).
    ///
    /// Takes no deploy lock: it never swaps, records, or reconciles, and
    /// the core's own stores are individually lock-safe. Planning from
    /// *inside* a deploy pass therefore works — a reconciliation entry may
    /// `Deploy::plan!` a candidate without the reentrancy hazard
    /// `Deploy::apply!` has (which re-enters the host's non-reentrant
    /// reconcile bracket).
    ///
    /// # Panics
    ///
    /// Panics if a runtime lock is poisoned.
    #[must_use]
    pub fn plan(&self, generation: &Functions, entry: &blake3::Hash) -> PlanReport {
        // Load additively, exactly as `deploy` does — content addressing
        // makes it inert, and validation reads the loaded view.
        self.load(generation);
        let problems = self.validate(generation, entry);
        let names = self.diff_bindings(&generation.bindings);
        PlanReport { names, problems }
    }

    /// Apply a generation: load, validate, swap, reconcile, report.
    ///
    /// `wire` runs on the freshly built entry VM before the entry is
    /// called — the client's hook for installing its own natives (the
    /// host binds the task runtime's `task_*` set here).
    ///
    /// # Errors
    ///
    /// [`DeployError::Validation`] if the generation is malformed (a
    /// binding or the entry references an object that is not loaded); the
    /// name table is untouched. [`DeployError::Entry`] if the entry
    /// function faults; the swap has already happened (see the error's
    /// docs).
    ///
    /// # Panics
    ///
    /// Panics if a runtime lock is poisoned.
    pub fn deploy(
        &self,
        generation: &Functions,
        entry: &blake3::Hash,
        wire: impl FnOnce(&mut Vm),
    ) -> Result<DeployReport, DeployError> {
        // 1. Load additively. Content addressing makes re-loading a known
        // hash a no-op, so this is idempotent and never destructive.
        self.load(generation);

        // 2. Validate against the cumulative view (a partial generation —
        // a future REPL one-item deploy — may bind names whose objects
        // arrived in an earlier generation).
        let problems = self.validate(generation, entry);
        if !problems.is_empty() {
            return Err(DeployError::Validation(problems));
        }

        // 3. Swap the name table atomically, diffing hash-against-hash,
        // and record the generation in the retirement ledger (recorded
        // at swap time, so a rejected deploy is never a generation).
        let (names, changed) = self.swap_names(&generation.bindings);
        let generation_id = {
            #[allow(clippy::unwrap_used)]
            self.ledger.lock().unwrap().record(generation)
        };

        // 4. Reconcile: run the entry on a fully loaded, client-wired VM.
        let mut vm = self.build_vm();
        wire(&mut vm);
        let value = match vm.call(entry, Vec::new()) {
            Ok(value) => value,
            Err(e) => return Err(DeployError::Entry(vm.runtime_error(e).to_string())),
        };

        // 5. Report, with diagnostics computed against the reconciled
        // registries (the entry's ensures/spawns have registered).
        let warnings = self.deploy_warnings(entry, &changed);
        Ok(DeployReport {
            value,
            names,
            generation: generation_id,
            warnings,
        })
    }

    /// Build a VM with every deployed object loaded: the factory's base
    /// natives plus the cumulative function/value/native stores.
    ///
    /// # Panics
    ///
    /// Panics if the object-store lock is poisoned.
    #[must_use]
    pub fn build_vm(&self) -> Vm {
        let mut vm = (self.vm_factory)();
        // Install `Live::latest`'s real resolution (uuid-keyed, so it
        // overwrites the factory's identity stub): hash → the name the ref
        // was deployed under → that name's current hash. VM-invoking, not
        // for reentrancy but for loading: a resolution can return a hash
        // from a generation deployed after this VM was last topped up (a
        // request racing a deploy), so a miss loads the cumulative stores
        // into the calling VM before the ref is handed to running code.
        let resolver = Arc::clone(&self.names);
        let loaded = Arc::clone(&self.loaded);
        vm.register_native_vm_impl(
            crate::native_uuid("live_latest"),
            Arc::new(move |vm, args| {
                let current = resolver.latest_native(args)?;
                if let Value::FunctionRef(hash) = &current
                    && !vm.has_function(hash)
                {
                    load_stores_into(&loaded, vm);
                }
                Ok(current)
            }),
        );
        // Install the `State` natives over their not-wired stubs, all
        // sharing this runtime's cell table — cells are present in every
        // VM the system builds. (Execute-sandbox VMs are built elsewhere
        // and keep the stubs: shipped-by-hash code gets no cells.)
        crate::state::register_state_natives(&mut vm, &self.cells);
        self.load_into(&mut vm);
        vm
    }

    /// Top up a VM with every deployed object. Loading is additive and
    /// content-addressed, so re-loading known hashes is a no-op — this
    /// is how a long-lived VM (a task's, iterating for weeks) picks up
    /// the objects later generations deployed. Pair with
    /// [`Self::epoch`] to skip the walk when nothing changed.
    ///
    /// # Panics
    ///
    /// Panics if the object-store lock is poisoned.
    pub fn load_into(&self, vm: &mut Vm) {
        load_stores_into(&self.loaded, vm);
    }

    /// The load epoch: bumped whenever a generation's objects are
    /// merged. Equal epochs guarantee a VM synced at the earlier read
    /// is missing nothing; a changed epoch says [`Self::load_into`] has
    /// something new. (Read it *before* the sync it guards, so a
    /// concurrent load re-syncs next time instead of being missed.)
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The loaded function at `hash`, from any generation ever deployed.
    ///
    /// # Panics
    ///
    /// Panics if the object-store lock is poisoned.
    #[must_use]
    pub fn lookup_function(&self, hash: &blake3::Hash) -> Option<Arc<CompiledFunction>> {
        self.lock_loaded().functions.get(hash).cloned()
    }

    /// Resolve a name against the current table.
    ///
    /// # Panics
    ///
    /// Panics if the name-table lock is poisoned.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<Binding> {
        self.name_table().get(name).cloned()
    }

    /// One `Live::latest` read: the current hash of the name `hash` was
    /// deployed under, or `hash` itself when it was never deployed under a
    /// name (or its names have diverged). This is exactly what the
    /// `live_latest` native returns inside a running program.
    ///
    /// # Panics
    ///
    /// Panics if the name-table lock is poisoned.
    #[must_use]
    pub fn latest(&self, hash: &blake3::Hash) -> blake3::Hash {
        self.names.latest(*hash)
    }

    /// An immutable snapshot of the current name table. Two lookups against
    /// one snapshot can never straddle a deploy.
    ///
    /// # Panics
    ///
    /// Panics if the name-table lock is poisoned.
    #[must_use]
    pub fn name_table(&self) -> Arc<NameTable> {
        Arc::clone(&self.names.snapshot().table)
    }

    /// Merge a generation's objects into the cumulative stores.
    fn load(&self, generation: &Generation) {
        let mut loaded = self.lock_loaded();
        for (hash, func) in &generation.functions {
            loaded
                .functions
                .entry(*hash)
                .or_insert_with(|| Arc::clone(func));
        }
        for (hash, value) in &generation.values {
            loaded.values.entry(*hash).or_insert_with(|| value.clone());
        }
        for (hash, native) in &generation.natives {
            loaded.natives.entry(*hash).or_insert(*native);
        }
        drop(loaded);
        self.epoch
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// Check every statically-named `State::init_versioned` obligation
    /// against the live cell table, *before* the name swap: a cell is
    /// fine when absent (`make` will create it), at the new type
    /// (adopt), or at the old type (the entry's perform migrates it);
    /// anything else rejects the deploy with the previous generation —
    /// and every cell — untouched.
    fn validate_migrations(&self, generation: &Generation, problems: &mut Vec<String>) {
        let mut migrations: Vec<&MigrationRecord> = generation.migrations.iter().collect();
        migrations.sort_by_key(|m| (&m.cell, &m.old, &m.new));
        for migration in migrations {
            let current = match self.cells.fingerprint(&migration.cell) {
                Ok(current) => current,
                Err(e) => {
                    problems.push(format!(
                        "cell `{}` could not be inspected: {e:?}",
                        migration.cell
                    ));
                    continue;
                }
            };
            if let Some(current) = current
                && current != migration.old
                && current != migration.new
            {
                problems.push(format!(
                    "cell `{}` is at type `{current}`, which is neither the pending \
                     migration's old type `{}` nor its new type `{}`",
                    migration.cell, migration.old, migration.new
                ));
            }
        }
    }

    /// Check a generation against the loaded view: the entry and every
    /// binding must resolve to a loaded object.
    fn validate(&self, generation: &Generation, entry: &blake3::Hash) -> Vec<String> {
        let loaded = self.lock_loaded();
        let known = |hash: &blake3::Hash| {
            loaded.functions.contains_key(hash)
                || loaded.values.contains_key(hash)
                || loaded.natives.contains_key(hash)
        };
        let mut problems = Vec::new();
        if !loaded.functions.contains_key(entry) {
            problems.push(format!("entry function {entry} is not a loaded function"));
        }
        self.validate_migrations(generation, &mut problems);
        let mut unbound: Vec<&Arc<str>> = generation
            .bindings
            .iter()
            .filter(|(_, binding)| !known(&binding.hash))
            .map(|(name, _)| name)
            .collect();
        unbound.sort();
        problems.extend(
            unbound
                .into_iter()
                .map(|name| format!("name `{name}` binds an object that is not loaded")),
        );
        problems
    }

    /// The name diff a generation *would* produce against the current
    /// table, computed read-only against an immutable snapshot — the
    /// dry-run half of [`Self::swap_names`]. Uses the same [`classify_name`]
    /// rule, so plan and the subsequent swap can never disagree on a name's
    /// fate.
    fn diff_bindings(&self, bindings: &HashMap<Arc<str>, Binding>) -> NameDiff {
        let snapshot = self.names.snapshot();
        let table = &snapshot.table;
        let mut diff = NameDiff::default();
        for (name, binding) in bindings {
            match classify_name(table.get(name), binding) {
                NameChange::Unchanged => diff.unchanged += 1,
                NameChange::Rebound => diff.rebound.push(Arc::clone(name)),
                NameChange::Retired => diff.retired.push(Arc::clone(name)),
                NameChange::Added => diff.added.push(Arc::clone(name)),
            }
        }
        diff.rebound.sort();
        diff.retired.sort();
        diff.added.sort();
        diff
    }

    /// Install the generation's bindings as the new name table — one
    /// atomic swap — and return the exact diff against the old table.
    ///
    /// The next table is the old one updated by the new bindings (a
    /// generation that omits a name leaves its old binding standing; a
    /// whole-build generation re-binds everything it still declares).
    ///
    /// This is where the **rebinding rule** applies: a changed name whose
    /// canonical signature is identical is a rebinding — its old hashes
    /// stay in the reverse map, resolving forward. A name whose signature
    /// changed, or is missing on either side (a producer that never
    /// rendered one — never a silent rebinding on missing data), is
    /// retire-and-fresh: every hash of its lineage leaves the reverse map,
    /// so old refs resolve to themselves.
    fn swap_names(
        &self,
        bindings: &HashMap<Arc<str>, Binding>,
    ) -> (NameDiff, Vec<crate::retire::ChangedName>) {
        #[allow(clippy::unwrap_used)]
        let mut state = self.names.state.write().unwrap();

        let mut diff = NameDiff::default();
        let mut changed = Vec::new();
        let mut table = (*state.table).clone();
        let mut reverse = state.reverse.clone();
        for (name, binding) in bindings {
            let prev = table.get(name);
            let change = classify_name(prev, binding);
            // The retirement ledger tracks every hash move (rebinding or
            // retirement); record it before the borrow of `table` ends.
            if let (Some(prev), NameChange::Rebound | NameChange::Retired) = (prev, change) {
                changed.push(crate::retire::ChangedName {
                    name: Arc::clone(name),
                    old: prev.hash,
                    new: binding.hash,
                });
            }
            match change {
                NameChange::Unchanged => {
                    diff.unchanged += 1;
                    // Same hash means same behavior and same type: keep the
                    // richer signature when this producer rendered none.
                    if binding.signature.is_some() {
                        table.insert(Arc::clone(name), binding.clone());
                    }
                }
                NameChange::Rebound => {
                    diff.rebound.push(Arc::clone(name));
                    table.insert(Arc::clone(name), binding.clone());
                }
                NameChange::Retired => {
                    diff.retired.push(Arc::clone(name));
                    for names in reverse.values_mut() {
                        names.remove(name);
                    }
                    reverse.retain(|_, names| !names.is_empty());
                    table.insert(Arc::clone(name), binding.clone());
                }
                NameChange::Added => {
                    diff.added.push(Arc::clone(name));
                    table.insert(Arc::clone(name), binding.clone());
                }
            }
            reverse
                .entry(binding.hash)
                .or_default()
                .insert(Arc::clone(name));
        }
        diff.rebound.sort();
        diff.retired.sort();
        diff.added.sort();

        *state = Arc::new(NameState {
            table: Arc::new(table),
            reverse,
        });
        (diff, changed)
    }

    fn lock_loaded(&self) -> std::sync::MutexGuard<'_, Loaded> {
        #[allow(clippy::unwrap_used)]
        self.loaded.lock().unwrap()
    }
}

/// Expand trace roots into per-root seed hashes for [`crate::retire::reach`],
/// pairing each with its origin (index-aligned).
fn root_seeds(
    roots: &[crate::retire::Root],
) -> (
    Vec<crate::retire::RootOrigin>,
    Vec<(usize, Vec<blake3::Hash>)>,
) {
    roots
        .iter()
        .enumerate()
        .map(|(index, root)| {
            let mut hashes = Vec::new();
            crate::retire::value_code_hashes(&root.value, &mut hashes);
            (root.origin.clone(), (index, hashes))
        })
        .unzip()
}

/// Top up a VM from the cumulative object stores (see
/// [`DeployRuntime::load_into`]). Free-standing so the per-VM
/// `live_latest` native can capture the `Arc`ed stores without a
/// [`DeployRuntime`] borrow.
fn load_stores_into(loaded: &Mutex<Loaded>, vm: &mut Vm) {
    #[allow(clippy::unwrap_used)]
    let loaded = loaded.lock().unwrap();
    for func in loaded.functions.values() {
        vm.load_function_shared(Arc::clone(func));
    }
    for (hash, value) in &loaded.values {
        vm.load_value(*hash, value.clone());
    }
    for (hash, (uuid, param_count)) in &loaded.natives {
        vm.load_native(*hash, *uuid, *param_count);
    }
}

/// The rebinding rule's comparison: both canonical signatures present and
/// byte-equal. The rendering is deterministic (position-canonical type
/// variables, uuid-keyed nominals, sorted ability rows — pinned by the
/// engine's byte-stability goldens), so string equality is canonical-form
/// equality. A renderer drift across versions fails safe: everything
/// compares "changed" and retires, and old refs resolve to themselves.
fn same_signature(prev: &Binding, next: &Binding) -> bool {
    match (&prev.signature, &next.signature) {
        (Some(prev), Some(next)) => prev == next,
        _ => false,
    }
}
