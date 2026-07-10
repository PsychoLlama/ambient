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
//!    generation's bindings — never a torn mixture.
//! 4. **Reconcile**: run the new entry function on a freshly built VM. The
//!    caller wires the VM first (a hook), which is how a client like the
//!    process runtime installs its own natives without this module knowing
//!    what a process is.
//! 5. **Report** the exact hash diff: names rebound, added, unchanged.
//!
//! This module is process-agnostic. The process runtime
//! ([`crate::process`]) is its first client; future frontends (REPL,
//! remote deploy) apply generations through the same operation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use ambient_ability::Value;
use ambient_engine::bytecode::CompiledFunction;
use ambient_engine::compiler::CompiledModule;
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
/// (hand-assembled generations in tests); every real build attaches it.
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

/// The exact name diff of one deploy, computed hash-against-hash.
#[derive(Debug, Default)]
pub struct NameDiff {
    /// Names bound to a different hash than before, sorted.
    pub rebound: Vec<Arc<str>>,
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
struct Loaded {
    functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,
    values: HashMap<blake3::Hash, Value>,
    natives: HashMap<blake3::Hash, (uuid::Uuid, u8)>,
}

/// The deploy core. One per running system; every frontend (dev loop,
/// REPL, remote) applies generations through [`Self::deploy`].
pub struct DeployRuntime {
    vm_factory: VmFactory,
    /// Cumulative object stores. Additive: a deploy only ever inserts.
    loaded: Mutex<Loaded>,
    /// The atomic name table. Swapped wholesale per deploy; readers clone
    /// the `Arc` and resolve against an immutable snapshot.
    names: RwLock<Arc<NameTable>>,
}

impl DeployRuntime {
    /// Create a deploy runtime with nothing loaded and an empty name table.
    #[must_use]
    pub fn new(vm_factory: VmFactory) -> Self {
        Self {
            vm_factory,
            loaded: Mutex::new(Loaded::default()),
            names: RwLock::new(Arc::new(NameTable::new())),
        }
    }

    /// Apply a generation: load, validate, swap, reconcile, report.
    ///
    /// `wire` runs on the freshly built entry VM before the entry is
    /// called — the client's hook for installing its own natives (the
    /// process runtime binds its `process_*` set here).
    ///
    /// # Errors
    ///
    /// [`DeployError::Validation`] if the generation is malformed (a
    /// binding or the entry references an object that is not loaded); the
    /// name table is untouched. [`DeployError::Entry`] if the entry
    /// function faults; the swap has already happened (see the error's
    /// docs).
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

        // 3. Swap the name table atomically, diffing hash-against-hash.
        let names = self.swap_names(&generation.bindings);

        // 4. Reconcile: run the entry on a fully loaded, client-wired VM.
        let mut vm = self.build_vm();
        wire(&mut vm);
        let value = match vm.call(entry, Vec::new()) {
            Ok(value) => value,
            Err(e) => return Err(DeployError::Entry(vm.runtime_error(e).to_string())),
        };

        // 5. Report.
        Ok(DeployReport { value, names })
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
        let loaded = self.lock_loaded();
        for func in loaded.functions.values() {
            vm.load_function_shared(Arc::clone(func));
        }
        for (hash, value) in &loaded.values {
            vm.load_value(*hash, value.clone());
        }
        for (hash, (uuid, param_count)) in &loaded.natives {
            vm.load_native(*hash, *uuid, *param_count);
        }
        vm
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

    /// An immutable snapshot of the current name table. Two lookups against
    /// one snapshot can never straddle a deploy.
    ///
    /// # Panics
    ///
    /// Panics if the name-table lock is poisoned.
    #[must_use]
    pub fn name_table(&self) -> Arc<NameTable> {
        #[allow(clippy::unwrap_used)]
        Arc::clone(&self.names.read().unwrap())
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

    /// Install the generation's bindings as the new name table — one
    /// atomic swap — and return the exact diff against the old table.
    ///
    /// The next table is the old one updated by the new bindings (a
    /// generation that omits a name leaves its old binding standing; a
    /// whole-build generation re-binds everything it still declares).
    fn swap_names(&self, bindings: &HashMap<Arc<str>, Binding>) -> NameDiff {
        #[allow(clippy::unwrap_used)]
        let mut names = self.names.write().unwrap();

        let mut diff = NameDiff::default();
        let mut next = (**names).clone();
        for (name, binding) in bindings {
            match next.insert(Arc::clone(name), binding.clone()) {
                Some(prev) if prev.hash == binding.hash => diff.unchanged += 1,
                Some(_) => diff.rebound.push(Arc::clone(name)),
                None => diff.added.push(Arc::clone(name)),
            }
        }
        diff.rebound.sort();
        diff.added.sort();

        *names = Arc::new(next);
        diff
    }

    fn lock_loaded(&self) -> std::sync::MutexGuard<'_, Loaded> {
        #[allow(clippy::unwrap_used)]
        self.loaded.lock().unwrap()
    }
}
