//! Native function bindings: the host side of `extern fn`.
//!
//! An `extern fn` declaration in Ambient source is a signature without a
//! body. The host — the engine itself for `core::`, an embedder for its own
//! module tree — provides the implementation through a [`NativeRegistry`]:
//! for each declaration, a binding keyed by *(module path, name)* supplies a
//! stable UUID and a Rust implementation.
//!
//! # Identity
//!
//! The UUID is the extern fn's **content identity**. An extern fn compiles
//! to a [`Native` object](crate::object::StoredObject::Native) whose
//! encoding is exactly `(uuid, param_count)`; callers link to that object's
//! hash like any function. The *(module path, name)* key is only the
//! compile-time attachment point — renaming or moving a declaration re-keys
//! the binding (a loud compile error until the host updates it) but never
//! changes a hash, because the name never enters the encoding. This mirrors
//! how the `Fqn` works everywhere else: a lookup key and post-hash label,
//! never a hashing input.
//!
//! The flip side is a discipline the host must keep: **a UUID pins a
//! meaning, forever**. Changing a bound function's semantics (or signature)
//! under the same UUID breaks every already-compiled caller silently; mint
//! a new UUID instead. This is the same rule extern *types* already live by.
//!
//! # Purity
//!
//! Native functions are pure value transformations
//! (`Vec<Value>` → `Value`): no host state, no effects. Effectful host
//! integration is what abilities are for — natives deliberately get no
//! ability channel, no store access, and no VM handle.
//!
//! # The contract
//!
//! [`verify_contract`](NativeRegistry::verify_contract) pins declarations
//! and bindings together in both directions: every `extern fn` in a
//! registered module must have a binding (with matching arity), and every
//! binding must name a declaration. A drifted core source or a stale
//! embedder table fails the build, not the first call.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::ast::ItemKind;
use crate::module_path::ModulePath;
use crate::{Value, VmError};

/// A native implementation: a pure function over runtime values.
pub type NativeFn = Arc<dyn Fn(Vec<Value>) -> Result<Value, VmError> + Send + Sync>;

/// The identity half of a binding: what the compiler needs to build the
/// [`Native` object](crate::object::StoredObject::Native) for a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeKey {
    /// The extern fn's stable content identity.
    pub uuid: Uuid,
    /// Declared parameter count. Recorded in the object encoding and
    /// validated against both the source declaration and the runtime
    /// implementation.
    pub arity: u8,
}

/// Host-provided implementations for the `extern fn` declarations of a
/// module tree, keyed two ways: by *(module path, name)* for compile-time
/// attachment, and by UUID for runtime dispatch.
#[derive(Default, Clone)]
pub struct NativeRegistry {
    by_path: HashMap<(ModulePath, Arc<str>), NativeKey>,
    by_uuid: HashMap<Uuid, NativeFn>,
}

impl std::fmt::Debug for NativeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `by_uuid` holds opaque closures; summarize by binding count.
        f.debug_struct("NativeRegistry")
            .field("bindings", &self.by_path.len())
            .finish_non_exhaustive()
    }
}

impl NativeRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind one `extern fn` declaration to its implementation.
    ///
    /// # Panics
    ///
    /// Panics on a duplicate *(module, name)* key or a duplicate UUID —
    /// both are host-side bugs (two bindings claiming one declaration, or
    /// one identity claiming two meanings), caught at registration rather
    /// than surfacing as nondeterministic dispatch.
    pub fn register(
        &mut self,
        module: &ModulePath,
        name: impl Into<Arc<str>>,
        uuid: Uuid,
        arity: u8,
        func: NativeFn,
    ) {
        let name = name.into();
        assert!(
            !self.by_uuid.contains_key(&uuid),
            "duplicate native uuid {uuid} (second binding: {module}::{name})"
        );
        let previous = self.by_path.insert(
            (module.clone(), Arc::clone(&name)),
            NativeKey { uuid, arity },
        );
        assert!(
            previous.is_none(),
            "duplicate native binding for {module}::{name}"
        );
        self.by_uuid.insert(uuid, func);
    }

    /// The binding for a declaration, by its compile-time key.
    #[must_use]
    pub fn key_for(&self, module: &ModulePath, name: &str) -> Option<NativeKey> {
        // Arc<str> keys hash like &str via Borrow, but the tuple key
        // doesn't; a lookup pair is cheap next to a compile.
        self.by_path
            .get(&(module.clone(), Arc::from(name)))
            .copied()
    }

    /// Every binding attached under `module`, as `(name, key)` pairs.
    #[must_use]
    pub fn keys_for_module(&self, module: &ModulePath) -> HashMap<Arc<str>, NativeKey> {
        self.by_path
            .iter()
            .filter(|((path, _), _)| path == module)
            .map(|((_, name), key)| (Arc::clone(name), *key))
            .collect()
    }

    /// The implementation registered for a UUID.
    #[must_use]
    pub fn impl_for(&self, uuid: &Uuid) -> Option<NativeFn> {
        self.by_uuid.get(uuid).cloned()
    }

    /// Iterate all `(uuid, impl)` pairs — how a VM inherits every
    /// registered implementation at construction.
    pub fn impls(&self) -> impl Iterator<Item = (Uuid, NativeFn)> + '_ {
        self.by_uuid.iter().map(|(uuid, f)| (*uuid, Arc::clone(f)))
    }

    /// Whether any bindings are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// Verify the total contract between declarations and bindings over a
    /// set of registered modules.
    ///
    /// Both directions are checked:
    ///
    /// 1. Every `extern fn` item in `modules` has a binding, with an arity
    ///    matching the declaration.
    /// 2. Every binding whose module path appears in `modules` names a
    ///    declaration that exists. (Bindings for unregistered module paths
    ///    are not flagged — an embedder may register bindings before
    ///    sources, or carry bindings for optional modules.)
    ///
    /// Returns every violation, so a drifted table reports completely
    /// rather than one item per build.
    #[must_use]
    pub fn verify_contract<'a>(
        &self,
        modules: impl Iterator<Item = (&'a ModulePath, &'a crate::ast::Module)>,
    ) -> Vec<ContractViolation> {
        let mut violations = Vec::new();
        let mut declared: HashMap<(ModulePath, Arc<str>), u8> = HashMap::new();

        for (path, module) in modules {
            for item in &module.items {
                let ItemKind::ExternFn(def) = &item.kind else {
                    continue;
                };
                #[allow(clippy::cast_possible_truncation)] // params are u8-bounded
                let arity = def.params.len() as u8;
                declared.insert((path.clone(), Arc::clone(&def.name)), arity);
                match self.by_path.get(&(path.clone(), Arc::clone(&def.name))) {
                    None => violations.push(ContractViolation::UnboundExtern {
                        module: path.clone(),
                        name: Arc::clone(&def.name),
                    }),
                    Some(key) if key.arity != arity => {
                        violations.push(ContractViolation::ArityMismatch {
                            module: path.clone(),
                            name: Arc::clone(&def.name),
                            declared: arity,
                            bound: key.arity,
                        });
                    }
                    Some(_) => {}
                }
            }
        }

        let declared_modules: std::collections::HashSet<&ModulePath> =
            declared.keys().map(|(path, _)| path).collect();
        for (path, name) in self.by_path.keys() {
            if declared_modules.contains(path)
                && !declared.contains_key(&(path.clone(), Arc::clone(name)))
            {
                violations.push(ContractViolation::DanglingBinding {
                    module: path.clone(),
                    name: Arc::clone(name),
                });
            }
        }

        violations.sort_by_key(ContractViolation::sort_key);
        violations
    }
}

impl NativeRegistry {
    /// Merge another registry's bindings into this one.
    ///
    /// # Panics
    ///
    /// Panics on duplicate keys or uuids, like [`Self::register`].
    pub fn merge(&mut self, other: &Self) {
        for ((module, name), key) in &other.by_path {
            // Registry invariant: every path key has an impl (register is
            // the only writer and inserts both).
            let Some(func) = other.by_uuid.get(&key.uuid).cloned() else {
                continue;
            };
            self.register(module, Arc::clone(name), key.uuid, key.arity, func);
        }
    }
}

mod core;

/// The engine's own native bindings: the implementations behind every
/// `extern fn` in the embedded `core_lib` sources. Built once.
///
/// Two consumers, one source: `register_core_modules` attaches these to the
/// build's [`crate::module_registry::ModuleRegistry`] (compile-time), and
/// [`crate::vm::Vm::new`] installs the implementations on every VM
/// (runtime) — so core natives are bound everywhere by construction,
/// isolated Execute VMs included (natives are pure, so granting them
/// unconditionally is sound).
#[must_use]
pub fn core_natives() -> &'static NativeRegistry {
    static CORE: std::sync::OnceLock<NativeRegistry> = std::sync::OnceLock::new();
    CORE.get_or_init(core::registry)
}

/// One way the declaration/binding contract can be broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractViolation {
    /// An `extern fn` declaration with no host binding.
    UnboundExtern { module: ModulePath, name: Arc<str> },
    /// A binding whose declared arity differs from the source signature.
    ArityMismatch {
        module: ModulePath,
        name: Arc<str>,
        declared: u8,
        bound: u8,
    },
    /// A binding naming a declaration that does not exist in its module.
    DanglingBinding { module: ModulePath, name: Arc<str> },
}

impl ContractViolation {
    fn sort_key(&self) -> String {
        match self {
            Self::UnboundExtern { module, name }
            | Self::ArityMismatch { module, name, .. }
            | Self::DanglingBinding { module, name } => format!("{module}::{name}"),
        }
    }
}

impl std::fmt::Display for ContractViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnboundExtern { module, name } => write!(
                f,
                "extern fn `{module}::{name}` has no native binding — the host must register one"
            ),
            Self::ArityMismatch {
                module,
                name,
                declared,
                bound,
            } => write!(
                f,
                "extern fn `{module}::{name}` declares {declared} parameter(s) \
                 but its native binding registers {bound}"
            ),
            Self::DanglingBinding { module, name } => write!(
                f,
                "native binding `{module}::{name}` names no extern fn declaration — \
                 remove the stale binding or restore the declaration"
            ),
        }
    }
}
