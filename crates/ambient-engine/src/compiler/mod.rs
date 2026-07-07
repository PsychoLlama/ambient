//! Compiler that transforms typed AST into bytecode.
//!
#![allow(clippy::cast_possible_truncation)]
//! This module implements the final stage of the Ambient compilation pipeline:
//! - Takes a type-checked AST (from `infer`)
//! - Emits bytecode instructions (using `bytecode::BytecodeBuilder`)
//! - Produces `CompiledFunction` values ready for the VM
//!
//! # Architecture
//!
//! ```text
//! Typed AST (Module)
//!       │
//!       ▼
//! ┌──────────────┐
//! │   Compiler   │ ─── Compiles each function/constant
//! └──────┬───────┘
//!        │
//!        ▼
//! CompiledModule { functions, entry_point }
//! ```
//!
//! # Module Organization
//!
//! - [`error`] - Compilation error types
//! - [`repl`] - REPL compilation support
//! - [`expr`] - Expression and statement compilation
//! - [`hash`] - Content-addressed hash computation

mod error;
mod expr;
mod hash;
pub(crate) mod intrinsics;
mod lambdas;
mod patterns;

pub use error::{CompileError, CompileErrorKind};

// Re-export for use by submodules
use expr::compile_expr;
use hash::{compute_temporary_hash, finalize_const_values, finalize_module_hashes};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{BindingId, FunctionDef, ImplDef, ImplMethod, ItemKind, Module};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, DebugInfo, Opcode};
use crate::fqn::{Fqn, ModuleId, NameKey};
use crate::value::Value;

// ─────────────────────────────────────────────────────────────────────────────
// Handler implicit parameter names
// ─────────────────────────────────────────────────────────────────────────────

/// Name for the implicit continuation parameter in handler functions (slot 0).
const HANDLER_PARAM_CONTINUATION: &str = "__continuation";

/// Name for the implicit suspended ability parameter in handler functions (slot 1).
const HANDLER_PARAM_SUSPENDED_ABILITY: &str = "__suspended_ability";

/// Helper to convert `Arc<str>` to `Value::String` (which uses `Arc<String>`).
fn str_to_value(s: &Arc<str>) -> Value {
    Value::String(Arc::new(s.to_string()))
}

/// Convert a span to (line, column) numbers.
///
/// Line and column are 1-indexed.
fn span_to_line_col(source: &str, span: crate::ast::Span) -> (u32, u32) {
    let offset = span.start as usize;
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

// ─────────────────────────────────────────────────────────────────────────────
// Compiled Module
// ─────────────────────────────────────────────────────────────────────────────

/// A compiled module containing all functions ready for execution.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// All compiled functions, keyed by their content-addressed hash.
    pub functions: HashMap<blake3::Hash, CompiledFunction>,

    /// Map from function names to their hashes.
    /// Does NOT include lambdas - they have no names.
    pub function_names: HashMap<Arc<str>, blake3::Hash>,

    /// Map from `const` names to their value-object hashes.
    ///
    /// Only local consts (an imported const is named in its own module).
    /// The hash addresses a [`StoredObject::Value`](crate::object::StoredObject::Value);
    /// these bind names the same way `function_names` do, so a const is a
    /// first-class named binding in the store's `names` index.
    pub const_names: HashMap<Arc<str>, blake3::Hash>,

    /// Map from lambda hashes to their parent function names.
    /// Used for navigation: to find a lambda's source location,
    /// compile the parent and match by hash.
    pub lambda_parents: HashMap<blake3::Hash, Arc<str>>,

    /// The entry point function (typically "run").
    pub entry_point: Option<blake3::Hash>,

    /// Canonical storage objects, keyed by object hash.
    ///
    /// Every function in `functions` is materialized from exactly one of
    /// these objects; recursive groups are stored as a single group object
    /// plus redirect stubs at each member hash. These are the bytes whose
    /// blake3 hash *is* the function identity — persist or transmit these,
    /// not the runtime `functions`.
    pub objects: HashMap<blake3::Hash, crate::object::StoredObject>,
}

impl CompiledModule {
    /// Create an empty compiled module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            function_names: HashMap::new(),
            const_names: HashMap::new(),
            lambda_parents: HashMap::new(),
            entry_point: None,
            objects: HashMap::new(),
        }
    }

    /// Get a function by name.
    #[must_use]
    pub fn get_function(&self, name: &str) -> Option<&CompiledFunction> {
        self.function_names
            .get(name)
            .and_then(|hash| self.functions.get(hash))
    }

    /// Get a function by hash.
    #[must_use]
    pub fn get_function_by_hash(&self, hash: &blake3::Hash) -> Option<&CompiledFunction> {
        self.functions.get(hash)
    }

    /// Merge another compiled module into this one.
    ///
    /// All functions from `other` are added to this module. If there are
    /// hash collisions (same function compiled identically), the existing
    /// function is kept. Name collisions are handled by keeping the first
    /// occurrence.
    pub fn merge(&mut self, other: &CompiledModule) {
        for (hash, func) in &other.functions {
            self.functions.entry(*hash).or_insert_with(|| func.clone());
        }
        for (name, hash) in &other.function_names {
            self.function_names.entry(Arc::clone(name)).or_insert(*hash);
        }
        for (name, hash) in &other.const_names {
            self.const_names.entry(Arc::clone(name)).or_insert(*hash);
        }
        for (hash, parent) in &other.lambda_parents {
            self.lambda_parents
                .entry(*hash)
                .or_insert_with(|| Arc::clone(parent));
        }
        for (hash, object) in &other.objects {
            self.objects.entry(*hash).or_insert_with(|| object.clone());
        }
        // Don't overwrite entry point if we already have one
        if self.entry_point.is_none() {
            self.entry_point = other.entry_point;
        }
    }

    /// Package this module as a runnable artifact pack: every canonical
    /// object plus the name bindings and entry point.
    #[must_use]
    pub fn to_pack(&self) -> crate::store::Pack {
        // Functions and consts share one flat name index; the object kind at
        // each hash (function vs `Value`) distinguishes them on the far side.
        let mut names: Vec<(String, blake3::Hash)> = self
            .function_names
            .iter()
            .chain(self.const_names.iter())
            .map(|(name, hash)| (name.to_string(), *hash))
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));

        // Redirects are derived from groups, so packs never carry them.
        let mut object_hashes: Vec<&blake3::Hash> = self
            .objects
            .iter()
            .filter(|(_, o)| !matches!(o, crate::object::StoredObject::Redirect { .. }))
            .map(|(h, _)| h)
            .collect();
        object_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

        crate::store::Pack {
            entry_point: self.entry_point,
            names,
            objects: object_hashes
                .iter()
                .map(|h| self.objects[*h].clone())
                .collect(),
        }
    }

    /// Reconstruct a runnable module from an artifact pack.
    ///
    /// Every function is materialized from its canonical object, so all
    /// hashes are recomputed from content — a tampered pack cannot smuggle
    /// code under a false hash.
    ///
    /// # Errors
    ///
    /// Returns an error if an object is malformed.
    pub fn from_pack(pack: &crate::store::Pack) -> Result<Self, crate::store::StoreError> {
        let mut module = Self::new();
        module.entry_point = pack.entry_point;

        for object in &pack.objects {
            if matches!(object, crate::object::StoredObject::Redirect { .. }) {
                // Legacy safety: packs shouldn't carry redirects, and one
                // without its group is meaningless. Regenerated below.
                continue;
            }
            let object_hash = object.hash();
            let materialized = object
                .materialize()
                .map_err(crate::store::StoreError::Object)?;
            let is_group =
                matches!(object, crate::object::StoredObject::Group(members) if members.len() > 1);
            for (index, (hash, func)) in materialized.into_iter().enumerate() {
                if is_group {
                    // Re-derive the redirect stubs a disk store needs to
                    // resolve member hashes back to their group.
                    module.objects.insert(
                        hash,
                        crate::object::StoredObject::Redirect {
                            group: object_hash,
                            index: index as u32,
                        },
                    );
                }
                module.functions.insert(hash, func);
            }
            module.objects.insert(object_hash, object.clone());
        }

        // Route each name to the right index by the kind of object it binds:
        // a `Value` object is a const, everything else a function.
        for (name, hash) in &pack.names {
            let is_const = matches!(
                module.objects.get(hash),
                Some(crate::object::StoredObject::Value(_))
            );
            let table = if is_const {
                &mut module.const_names
            } else {
                &mut module.function_names
            };
            table.insert(Arc::from(name.as_str()), *hash);
        }

        Ok(module)
    }
}

impl Default for CompiledModule {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Compiler State
// ─────────────────────────────────────────────────────────────────────────────

/// Compiler state for a single function.
struct FunctionCompiler {
    /// Bytecode builder.
    builder: BytecodeBuilder,

    /// Map from binding IDs to local slots.
    locals: HashMap<BindingId, u16>,

    /// Map from local variable names to their slots.
    /// This is used when lowering doesn't produce Local(id) references.
    local_names: HashMap<Arc<str>, u16>,

    /// Next available local slot.
    next_local: u16,

    /// Map from function names to their hashes (for recursive calls).
    function_hashes: HashMap<NameKey, blake3::Hash>,

    /// Captured variables (for closures): binding ID -> capture slot index.
    /// These are variables from enclosing scopes that this function captures.
    captures: HashMap<BindingId, u16>,

    /// Captured variable names (for closures).
    capture_names: HashMap<Arc<str>, u16>,

    /// Parent's locals - used during closure compilation to identify free variables.
    /// Maps binding IDs from the enclosing scope to their local slots there.
    parent_locals: Option<HashMap<BindingId, u16>>,

    /// Parent's local names - for name-based lookups during closure compilation.
    parent_local_names: Option<HashMap<Arc<str>, u16>>,

    /// Block-scoped `const` names in scope, mapped to their value object's
    /// content hash. A reference emits `LoadObject` of the hash (never a
    /// local slot) — a `const` is a compile-time value, not a runtime local.
    /// Inherited into a nested lambda at creation, since the hash is
    /// position-independent and an enclosing const is visible in the closure.
    block_consts: HashMap<Arc<str>, blake3::Hash>,

    /// Debug information being built.
    debug_info: DebugInfo,
}

impl FunctionCompiler {
    /// Create a new function compiler.
    fn new(function_hashes: HashMap<NameKey, blake3::Hash>) -> Self {
        Self {
            builder: BytecodeBuilder::new(),
            locals: HashMap::new(),
            local_names: HashMap::new(),
            next_local: 0,
            function_hashes,
            captures: HashMap::new(),
            capture_names: HashMap::new(),
            parent_locals: None,
            parent_local_names: None,
            block_consts: HashMap::new(),
            debug_info: DebugInfo::new(),
        }
    }

    /// Create a function compiler for a closure, with access to parent scope.
    fn new_for_closure(
        function_hashes: HashMap<NameKey, blake3::Hash>,
        parent_locals: HashMap<BindingId, u16>,
        parent_local_names: HashMap<Arc<str>, u16>,
        parent_block_consts: HashMap<Arc<str>, blake3::Hash>,
    ) -> Self {
        Self {
            builder: BytecodeBuilder::new(),
            locals: HashMap::new(),
            local_names: HashMap::new(),
            next_local: 0,
            function_hashes,
            captures: HashMap::new(),
            capture_names: HashMap::new(),
            parent_locals: Some(parent_locals),
            parent_local_names: Some(parent_local_names),
            // A lambda inherits the enclosing block consts in scope; each is a
            // bare `LoadObject` of a hash, so no capture slot is needed.
            block_consts: parent_block_consts,
            debug_info: DebugInfo::new(),
        }
    }

    /// Record a source mapping for the current bytecode position.
    ///
    /// This associates the current bytecode offset with the given source span.
    /// Line and column are set to 0 initially; they can be computed later when
    /// the source code is available.
    fn record_span(&mut self, span: crate::ast::Span) {
        let bytecode_offset = self.builder.bytecode().len();
        self.debug_info.add_mapping(
            bytecode_offset,
            span.start as usize,
            span.end as usize,
            0, // Line will be computed later if source is provided
            0, // Column will be computed later if source is provided
        );
    }

    /// Record a local variable name for debug output.
    fn record_local_name(&mut self, slot: u16, name: &str) {
        self.debug_info.add_local_name(slot, name);
    }

    /// Allocate a local slot for a binding with a name.
    fn alloc_local_with_name(
        &mut self,
        id: BindingId,
        name: &Arc<str>,
    ) -> Result<u16, CompileError> {
        if self.next_local == u16::MAX {
            return Err(CompileError::new(
                CompileErrorKind::TooManyLocals {
                    count: self.next_local as usize + 1,
                },
                (0, 0),
            ));
        }
        let slot = self.next_local;
        self.next_local += 1;
        self.locals.insert(id, slot);
        self.local_names.insert(Arc::clone(name), slot);
        // Record the name for debug info
        self.record_local_name(slot, name);
        Ok(slot)
    }

    /// Get the local slot for a binding by ID.
    fn get_local(&self, id: BindingId, span: (u32, u32)) -> Result<u16, CompileError> {
        self.locals
            .get(&id)
            .copied()
            .ok_or_else(|| CompileError::new(CompileErrorKind::UndefinedLocal { id }, span))
    }

    /// Get the local slot for a binding by name.
    fn get_local_by_name(&self, name: &str) -> Option<u16> {
        self.local_names.get(name).copied()
    }

    /// Check if a binding ID is from the parent scope (needs to be captured).
    fn is_parent_binding(&self, id: BindingId) -> bool {
        if let Some(parent) = &self.parent_locals {
            parent.contains_key(&id) && !self.locals.contains_key(&id)
        } else {
            false
        }
    }

    /// Check if a name is from the parent scope (needs to be captured).
    fn is_parent_name(&self, name: &str) -> bool {
        if let Some(parent) = &self.parent_local_names {
            parent.contains_key(name) && !self.local_names.contains_key(name)
        } else {
            false
        }
    }

    /// Get or create a capture slot for a parent binding.
    fn get_or_create_capture(&mut self, id: BindingId, name: Arc<str>) -> u16 {
        if let Some(&slot) = self.captures.get(&id) {
            slot
        } else {
            let slot = self.captures.len() as u16;
            self.captures.insert(id, slot);
            self.capture_names.insert(name, slot);
            slot
        }
    }

    /// Get or create a capture slot for a parent name.
    fn get_or_create_capture_by_name(&mut self, name: Arc<str>) -> u16 {
        if let Some(&slot) = self.capture_names.get(&name) {
            slot
        } else {
            // Use capture_names.len() since we're tracking by name
            let slot = self.capture_names.len() as u16;
            self.capture_names.insert(name, slot);
            slot
        }
    }

    /// Get the list of captured names in capture slot order.
    fn get_capture_names_in_order(&self) -> Vec<(Arc<str>, u16)> {
        let mut captures: Vec<_> = self
            .capture_names
            .iter()
            .map(|(name, &slot)| (Arc::clone(name), slot))
            .collect();
        captures.sort_by_key(|(_, slot)| *slot);
        captures
    }
}

/// Context for module compilation that accumulates lambda functions.
struct ModuleContext {
    /// Lambda functions discovered during compilation.
    /// Maps (temporary hash, parent function name) to compiled function.
    lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
    /// Counter for generating unique lambda temporary hashes.
    lambda_counter: u32,
    /// Value objects for block-scoped `const`s discovered while compiling
    /// function bodies. Folded into the module's objects after finalization
    /// (deduplicated by hash), alongside module-level const objects. Module
    /// consts are content-addressed in a pre-pass instead; these can only be
    /// found by walking bodies.
    const_objects: HashMap<blake3::Hash, crate::object::StoredObject>,
    /// Name of the function currently being compiled.
    /// Used to track lambda parent relationships.
    current_function: Option<Arc<str>>,
    /// Enum variant constructors in scope: variant name → variant info.
    /// Seeded with the prelude (Option/Result); local enum declarations
    /// shadow prelude variants of the same name.
    enums: HashMap<Arc<str>, VariantInfo>,
    /// Foreign enum variant constructors keyed by their canonical
    /// two-segment [`Fqn`] (`core::Option::Some`,
    /// `pkg::shapes::Shape::Circle`). Consulted *before* the bare
    /// [`Self::enums`] table so a fully-qualified reference always inlines
    /// the defining enum's tag, never a same-named local variant's (see
    /// `CompileOptions::foreign_enum_variants`).
    foreign_variants: HashMap<Fqn, VariantInfo>,
    /// Unit-struct constructors in scope, keyed by resolution key: local
    /// declarations by bare name ([`NameKey::Bare`]), foreign ones by their
    /// [`Fqn`] ([`NameKey::Item`]). A reference whose key is here compiles
    /// to an empty record value, mirroring a nullary enum variant.
    unit_structs: HashSet<NameKey>,
    /// Module-level constants: resolution key → the content hash of the
    /// `const`'s value object. A reference to one compiles to a `LoadObject`
    /// of this hash, so a constant links through the same name→hash table as
    /// a function — deduplicated and hash-addressed, not inlined. Local
    /// constants key on their `Fqn` (bare in the registry-less convention),
    /// imported ones by their `Fqn`; both hashes are final (value objects are
    /// leaves computed in a pre-pass). Key present ⇒ the name is a const.
    const_hashes: HashMap<NameKey, blake3::Hash>,
    /// Module-declared abilities in scope: ability name → compile info.
    /// The identity comes from the type checker (`AbilityDef::resolved_id`);
    /// the compiler never re-derives interface hashes. Local declarations
    /// resolve bare, so this stays a bare-name table.
    abilities: HashMap<Arc<str>, CompiledAbilityInfo>,
    /// Prelude abilities (embedder-resolved declaration modules, e.g. the
    /// platform bindings interface), kept apart from locals so a local
    /// declaration and a namespaced prelude ability of the same name
    /// resolve independently. Keyed by bare name (the `platform` module
    /// never compiles, so it has no `Fqn` linking entry).
    prelude_abilities: HashMap<Arc<str>, CompiledAbilityInfo>,
    /// Foreign ability identities keyed by their [`Fqn`]; see
    /// [`CompileOptions::foreign_abilities`].
    foreign_abilities: HashMap<Fqn, CompiledAbilityInfo>,
    /// The current module's identity (registry-less compiles pass `None`).
    /// The module's own consts and unit structs key on their `Fqn` under
    /// this id, matching the resolve pass; a same-module ability reference
    /// arrives as `Item(Fqn)` and resolves back to the bare-keyed local
    /// ability table via this id.
    module_id: Option<ModuleId>,
}

/// Compile-time info for one module-declared ability.
#[derive(Debug, Clone)]
pub(crate) struct CompiledAbilityInfo {
    pub id: crate::types::AbilityId,
    /// Method names in declaration order; a method's ID is its index.
    pub methods: Vec<Arc<str>>,
}

impl CompiledAbilityInfo {
    /// Method ID (declaration index) for a method name.
    pub(crate) fn method_id(&self, name: &str) -> Option<u16> {
        #[allow(clippy::cast_possible_truncation)]
        self.methods
            .iter()
            .position(|m| m.as_ref() == name)
            .map(|idx| idx as u16)
    }
}

/// Compile-time info for one enum variant constructor.
#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub enum_name: Arc<str>,
    pub tag: u16,
    pub has_payload: bool,
}

impl ModuleContext {
    fn new(module_id: Option<ModuleId>) -> Self {
        // Option/Result carry no hardcoded seed: they arrive through the same
        // `imported_enums` channel as any other enum, folded in from the
        // prelude by `build_imported_enums`. A registry-less compile (no
        // prelude) therefore starts with no enums, exactly like the checker.
        Self {
            lambdas: Vec::new(),
            lambda_counter: 0,
            const_objects: HashMap::new(),
            current_function: None,
            enums: HashMap::new(),
            foreign_variants: HashMap::new(),
            unit_structs: HashSet::new(),
            const_hashes: HashMap::new(),
            abilities: HashMap::new(),
            prelude_abilities: HashMap::new(),
            foreign_abilities: HashMap::new(),
            module_id,
        }
    }

    /// Register content hashes for module-level constants so a reference to
    /// one compiles to a `LoadObject` of its value object (see the
    /// `ExprKind::Name` arm in `expr.rs`). Local hashes come from the const
    /// pre-pass ([`finalize_const_values`]); imported hashes are final
    /// already (a value object is a leaf). Keys never collide: locals key on
    /// their own `Fqn`, imports on the defining module's `Fqn`.
    fn register_const_hashes(&mut self, hashes: &HashMap<NameKey, blake3::Hash>) {
        for (key, hash) in hashes {
            self.const_hashes.insert(key.clone(), *hash);
        }
    }

    /// The content hash of the module-level constant a reference resolves to,
    /// if any. A present key means the name denotes a `const`.
    fn constant_hash(&self, key: &NameKey) -> Option<blake3::Hash> {
        self.const_hashes.get(key).copied()
    }

    /// Register prelude abilities (declaration modules resolved by the
    /// embedder, e.g. the `platform` bindings interface) so ability calls
    /// and handler literals compile against their content-hash
    /// identities.
    fn register_prelude_abilities(
        &mut self,
        prelude: &[std::sync::Arc<crate::ability_resolver::DynAbility>],
    ) {
        for ability in prelude {
            self.prelude_abilities.insert(
                Arc::clone(&ability.name),
                CompiledAbilityInfo {
                    id: ability.id,
                    methods: ability
                        .methods
                        .iter()
                        .map(|m| Arc::clone(&m.name))
                        .collect(),
                },
            );
        }
    }

    /// Resolve an ability name against locals and the prelude.
    ///
    /// Mirrors the type checker's namespace policy (which already gated
    /// correctness): namespace-qualified references name the prelude,
    /// bare references name locals. Bare builtins (Exception) are not
    /// here — callers fall back to the core-ability tables for those.
    /// Resolve an ability reference to its compile-time info.
    ///
    /// Bare unresolved references are local declarations. Resolved or
    /// path-qualified references try the canonical foreign channel first,
    /// then the prelude (platform declarations, keyed by bare name — the
    /// `platform` module never compiles, so it has no canonical entry).
    fn resolve_ability(&self, ability: &crate::ast::QualifiedName) -> Option<&CompiledAbilityInfo> {
        match &ability.resolved {
            // A resolved reference names a same-module ability (its `Fqn`'s
            // module is ours — look it up bare in the local table), a
            // foreign ability by its `Fqn`, or a platform prelude ability by
            // bare name (the `platform` module never compiles, so it has no
            // `Fqn` linking entry).
            Some(fqn) => self
                .module_id
                .as_ref()
                .filter(|id| **id == fqn.module)
                .and_then(|_| self.abilities.get(fqn.name()))
                .or_else(|| self.foreign_abilities.get(fqn))
                .or_else(|| self.prelude_abilities.get(ability.resolved_name())),
            // A bare unresolved reference is a local declaration
            // (same-module abilities are never resolved to an `Fqn`). A
            // *path-qualified* unresolved reference means the resolve pass
            // did not run (e.g. the platform-declaration compile path): it
            // names a prelude ability by its spelled final segment.
            None if ability.path.is_empty() => self.abilities.get(&ability.name),
            None => self.prelude_abilities.get(&ability.name),
        }
    }

    /// Register the foreign-ability channel (see
    /// [`CompileOptions::foreign_abilities`]).
    fn register_foreign_abilities(
        &mut self,
        foreign: &[(Fqn, std::sync::Arc<crate::ability_resolver::DynAbility>)],
    ) {
        for (fqn, dyn_ab) in foreign {
            self.foreign_abilities.insert(
                fqn.clone(),
                CompiledAbilityInfo {
                    id: dyn_ab.id,
                    methods: dyn_ab.methods.iter().map(|m| Arc::clone(&m.name)).collect(),
                },
            );
        }
    }

    /// Register a module's `ability` declarations from their type-checked
    /// identities.
    fn register_abilities(&mut self, module: &Module) -> Result<(), CompileError> {
        for item in &module.items {
            if let ItemKind::Ability(def) = &item.kind {
                let Some(id) = def.resolved_id else {
                    return Err(CompileError::new(
                        CompileErrorKind::Internal {
                            message: "ability declaration missing resolved identity",
                        },
                        (def.name_span.start, def.name_span.end),
                    ));
                };
                self.abilities.insert(
                    Arc::clone(&def.name),
                    CompiledAbilityInfo {
                        id,
                        methods: def.methods.iter().map(|m| Arc::clone(&m.name)).collect(),
                    },
                );
            }
        }
        Ok(())
    }

    /// Look up an ability by identity (for handler literals, where the
    /// type checker hands the compiler an `AbilityId`). Searches locals,
    /// the prelude, then imported foreign abilities — the same set
    /// [`Self::resolve_ability`] covers, so an ability reachable by name is
    /// also reachable by id.
    fn ability_by_id(
        &self,
        id: crate::types::AbilityId,
    ) -> Option<(&Arc<str>, &CompiledAbilityInfo)> {
        self.abilities
            .iter()
            .find(|(_, info)| info.id == id)
            .or_else(|| {
                self.prelude_abilities
                    .iter()
                    .find(|(_, info)| info.id == id)
            })
            .or_else(|| {
                self.foreign_abilities
                    .iter()
                    .find(|(_, info)| info.id == id)
                    // An `Fqn`'s ident path always has a final segment (the
                    // ability's own name).
                    .and_then(|(fqn, info)| fqn.ident.last().map(|name| (name, info)))
            })
    }

    /// Register one enum definition's variant constructors.
    fn register_enum_def(&mut self, enum_def: &crate::ast::EnumDef) {
        for (idx, variant) in enum_def.variants.iter().enumerate() {
            self.enums.insert(
                Arc::clone(&variant.name),
                VariantInfo {
                    enum_name: Arc::clone(&enum_def.name),
                    #[allow(clippy::cast_possible_truncation)]
                    tag: idx as u16,
                    has_payload: variant.payload.is_some(),
                },
            );
        }
    }

    /// Register imported enum definitions (from `use pkg::m::{SomeEnum}`).
    /// Runs before [`Self::register_enums`], so local declarations shadow
    /// imported variants, which shadow the prelude — the same precedence
    /// the type checker applies.
    fn register_imported_enums(&mut self, imported: &[crate::ast::EnumDef]) {
        for enum_def in imported {
            self.register_enum_def(enum_def);
        }
    }

    /// Register foreign enum variant constructors under their canonical
    /// two-segment [`Fqn`] (see [`CompileOptions::foreign_enum_variants`]).
    /// A separate table from [`Self::enums`]: it is keyed by `Fqn`, not
    /// bare name, so a qualified reference is never shadowed by (nor
    /// shadows) a same-named local variant.
    fn register_foreign_variants(&mut self, variants: &[(Fqn, VariantInfo)]) {
        for (fqn, info) in variants {
            self.foreign_variants.insert(fqn.clone(), info.clone());
        }
    }

    /// Register a module's enum declarations, shadowing prelude variants.
    fn register_enums(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Enum(enum_def) = &item.kind {
                self.register_enum_def(enum_def);
            }
        }
    }

    /// Register the local module's unit structs under their `Fqn` (or bare,
    /// registry-less) — their resolution key when referenced from within
    /// the module.
    fn register_unit_structs(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Struct(s) = &item.kind
                && s.is_unit_value()
            {
                self.unit_structs
                    .insert(own_item_key(self.module_id.as_ref(), &s.name));
            }
        }
    }

    /// Register foreign unit structs under their [`Fqn`] keys — the key an
    /// imported or fully-qualified value reference resolves to (see
    /// `build::build_foreign_unit_structs`).
    fn register_imported_unit_structs(&mut self, keys: &[NameKey]) {
        for key in keys {
            self.unit_structs.insert(key.clone());
        }
    }

    /// Set the current function being compiled.
    fn set_current_function(&mut self, name: Arc<str>) {
        self.current_function = Some(name);
    }

    /// Generate a unique temporary hash for a lambda.
    fn next_lambda_hash(&mut self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"__lambda__");
        hasher.update(&self.lambda_counter.to_le_bytes());
        self.lambda_counter += 1;
        hasher.finalize()
    }

    /// Register a compiled lambda and return its temporary hash.
    /// The lambda is associated with the current function being compiled.
    fn register_lambda(&mut self, function: CompiledFunction) -> blake3::Hash {
        let hash = self.next_lambda_hash();
        let parent = self
            .current_function
            .clone()
            .unwrap_or_else(|| Arc::from("__unknown__"));
        self.lambdas.push((hash, parent, function));
        hash
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Module Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Options for module compilation.
///
/// The zero-value default compiles without debug info, imports, or
/// prelude abilities.
#[derive(Default)]
pub struct CompileOptions<'a> {
    /// The current module's identity. When set, the module's own items
    /// (functions, consts, unit structs, abilities) key on their
    /// [`Fqn`] — matching the resolve pass, which canonicalizes every
    /// same-module reference to `Fqn(module_id, [name])`. `None` is the
    /// registry-less convention (single-file/REPL-less unit compiles): the
    /// resolve pass never ran, so same-module references stay bare and own
    /// items key bare to match.
    pub module_id: Option<ModuleId>,
    /// Original source code, for debug info (line/column mapping).
    pub source: Option<&'a str>,
    /// Source file path, for display in stack traces.
    pub source_file: Option<&'a str>,
    /// Imported function names mapped to their content-addressed hashes.
    pub imported_hashes: Option<HashMap<NameKey, blake3::Hash>>,
    /// Imported enum definitions (`use pkg::m::{SomeEnum}`). Constructors
    /// inline by tag rather than linking by hash, so the compiler needs
    /// the definitions themselves, not name→hash entries.
    pub imported_enums: Vec<crate::ast::EnumDef>,
    /// Foreign unit structs, as canonical `<module>::Origin` keys. A unit
    /// struct compiles to an empty record value, so only its key is needed
    /// (not the definition), keyed like foreign constants. The resolve pass
    /// rewrites every cross-module unit-struct value reference to its
    /// canonical key, which is looked up here.
    pub imported_unit_structs: Vec<NameKey>,
    /// Foreign constant content hashes, keyed by canonical `Fqn`
    /// ([`NameKey::Item`]). A `const` links by hash exactly like a function:
    /// the resolve pass rewrites every cross-module constant reference to its
    /// canonical key, and a reference to a key present here compiles to a
    /// `LoadObject` of that value object (never an inlined literal). The
    /// defining module emits the value object itself, so only the hash is
    /// needed here — a name→hash channel like [`Self::imported_hashes`], not
    /// an AST-replication one.
    pub imported_const_hashes: HashMap<NameKey, blake3::Hash>,
    /// Foreign enum variant constructors, keyed by their canonical
    /// two-segment [`Fqn`] (`core::Option::Some`,
    /// `pkg::shapes::Shape::Circle`). Variant construction inlines a
    /// `MakeEnum` tag rather than linking by hash, so — like
    /// [`Self::imported_enums`] — the compiler needs the tag/payload layout,
    /// not a name→hash entry. This is the *qualified* channel:
    /// [`Self::imported_enums`] covers bare/enum-imported variants;
    /// fully-qualified references resolve to an `Fqn` looked up here.
    pub foreign_enum_variants: Vec<(Fqn, VariantInfo)>,
    /// Prelude abilities (embedder-resolved declaration modules, e.g. the
    /// platform bindings interface). Local declarations shadow them.
    pub prelude_abilities: &'a [std::sync::Arc<crate::ability_resolver::DynAbility>],
    /// Foreign ability identities, keyed by their [`Fqn`]. The resolve pass
    /// rewrites cross-module ability references to these keys; identities
    /// come from the checker (the content-addressed interface hash), never
    /// re-derived here.
    pub foreign_abilities: Vec<(Fqn, std::sync::Arc<crate::ability_resolver::DynAbility>)>,
}

/// Compile a module to bytecode.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module(module: &Module) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, CompileOptions::default())
}

/// Compile a module with explicit [`CompileOptions`].
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module_with_options(
    module: &Module,
    options: CompileOptions,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, options)
}

/// Compile a module with imported function references.
///
/// This is used for cross-module compilation where the module imports
/// functions from other already-compiled modules.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `imported_hashes` - Map from imported function names to their content-addressed hashes
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_module_with_imports(
    module: &Module,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            imported_hashes: Some(imported_hashes),
            ..CompileOptions::default()
        },
    )
}

/// Compile a module to bytecode with debug information.
///
/// When `source` and `source_file` are provided, the compiled functions will
/// include debug info mapping bytecode offsets to source locations.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `source` - The original source code (for computing line/column info)
/// * `source_file` - The path to the source file (for display in stack traces)
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module_with_source(
    module: &Module,
    source: &str,
    source_file: &str,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            source: Some(source),
            source_file: Some(source_file),
            ..CompileOptions::default()
        },
    )
}

/// Compile a module with imported function references and debug information.
///
/// This combines cross-module compilation with debug info.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `source` - The original source code (for computing line/column info)
/// * `source_file` - The path to the source file (for display in stack traces)
/// * `imported_hashes` - Map from imported function names to their content-addressed hashes
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_module_with_imports_and_source(
    module: &Module,
    source: &str,
    source_file: &str,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            source: Some(source),
            source_file: Some(source_file),
            imported_hashes: Some(imported_hashes),
            ..CompileOptions::default()
        },
    )
}

/// The lookup key for one of the current module's own items (function,
/// const, unit struct), matching the resolve pass: `Item(Fqn(module_id,
/// [name]))` when the module has an identity, else bare (registry-less).
fn own_item_key(module_id: Option<&ModuleId>, name: &Arc<str>) -> NameKey {
    match module_id {
        Some(id) => NameKey::Item(Fqn::new(id.clone(), vec![Arc::clone(name)])),
        None => NameKey::Bare(Arc::clone(name)),
    }
}

/// Implementation of module compilation with optional debug info.
#[allow(clippy::too_many_lines)]
fn compile_module_impl(
    module: &Module,
    options: CompileOptions,
) -> Result<CompiledModule, CompileError> {
    let CompileOptions {
        module_id,
        source,
        source_file,
        imported_hashes,
        imported_enums,
        imported_unit_structs,
        imported_const_hashes,
        foreign_enum_variants,
        prelude_abilities,
        foreign_abilities,
    } = options;
    // Collect function definitions.
    let functions: Vec<&FunctionDef> = module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Function(func) = &item.kind {
                Some(func)
            } else {
                None
            }
        })
        .collect();

    // Phase 1: Create temporary hashes for name-based lookup during compilation.
    // These will be replaced with content-addressed hashes after compilation.
    // Start with imported hashes (these are already content-addressed).
    let mut temp_hashes: HashMap<NameKey, blake3::Hash> = imported_hashes.unwrap_or_default();

    // Add temporary hashes for local functions, keyed on the same identity
    // the resolve pass gives a same-module reference: `Item(Fqn)` when the
    // module has an identity, bare in the registry-less convention.
    for func in &functions {
        let hash = compute_temporary_hash(&func.name);
        temp_hashes.insert(own_item_key(module_id.as_ref(), &func.name), hash);
    }

    // Content-address every local `const` in a pre-pass, before function
    // bodies compile, so a reference can link to the value object's final
    // hash (see `finalize_const_values`). A `const` is not compiled to a
    // function — it is a standalone leaf value object — so it needs no temp
    // hash or call-graph pass.
    let local_consts: Vec<(NameKey, Value)> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Const(const_def) => {
                let value = crate::const_eval::literal_value(&const_def.value)?;
                Some((own_item_key(module_id.as_ref(), &const_def.name), value))
            }
            _ => None,
        })
        .collect();
    let (local_const_hashes, const_objects) = finalize_const_values(&local_consts);

    // Bind each local const's short name to its value-object hash, so a
    // named const is a first-class binding in the store's `names` index
    // (imported consts are named in their own module).
    let const_names: HashMap<Arc<str>, blake3::Hash> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Const(const_def) => {
                let key = own_item_key(module_id.as_ref(), &const_def.name);
                let hash = local_const_hashes.get(&key)?;
                Some((Arc::clone(&const_def.name), *hash))
            }
            _ => None,
        })
        .collect();

    // Add temporary hashes for impl methods under their canonical symbols.
    // Impl methods are ordinary functions named by `types::impl_method_symbol`;
    // method-call sites resolve these symbols through the same name→hash table
    // as regular calls, and hash finalization content-addresses them together
    // with everything else.
    let impl_methods: Vec<(&ImplDef, &ImplMethod)> = module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Impl(impl_def) = &item.kind {
                Some(impl_def.methods.iter().map(move |m| (impl_def, m)))
            } else {
                None
            }
        })
        .flatten()
        .collect();

    for (_, method) in &impl_methods {
        let Some(symbol) = &method.resolved_symbol else {
            return Err(CompileError::new(
                CompileErrorKind::Internal {
                    message: "impl method missing resolved symbol",
                },
                (method.span.start, method.span.end),
            ));
        };
        temp_hashes.insert(
            NameKey::Bare(Arc::clone(symbol)),
            compute_temporary_hash(symbol),
        );
    }

    // Create module context for tracking lambdas discovered during
    // compilation, with the module's enum constructors registered.
    let mut ctx = ModuleContext::new(module_id.clone());
    ctx.register_imported_enums(&imported_enums);
    ctx.register_enums(module);
    ctx.register_foreign_variants(&foreign_enum_variants);
    ctx.register_imported_unit_structs(&imported_unit_structs);
    ctx.register_unit_structs(module);
    // Both local (pre-pass) and imported const hashes are final and link the
    // same way; a reference to either key emits a `LoadObject`.
    ctx.register_const_hashes(&local_const_hashes);
    ctx.register_const_hashes(&imported_const_hashes);
    ctx.register_prelude_abilities(prelude_abilities);
    ctx.register_foreign_abilities(&foreign_abilities);
    ctx.register_abilities(module)?;

    // Phase 2: Compile each function using temporary hashes.
    let mut compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)> = Vec::new();
    for func in &functions {
        // Track current function for lambda parent relationships.
        ctx.set_current_function(Arc::clone(&func.name));
        let compiled =
            compile_function_with_hash(func, &temp_hashes, &mut ctx, source, source_file)?;
        let is_main = &*func.name == "run";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Constants compile to standalone value objects (folded into the module
    // below), not functions; each reference is a `LoadObject` of the object's
    // hash (see the `ExprKind::Name` arm).

    // Compile impl methods as ordinary named functions.
    for (impl_def, method) in &impl_methods {
        // Presence was validated when building temp_hashes above.
        let Some(symbol) = &method.resolved_symbol else {
            continue;
        };
        ctx.set_current_function(Arc::clone(symbol));
        let compiled = compile_impl_method(
            impl_def,
            method,
            &temp_hashes,
            &mut ctx,
            source,
            source_file,
        )?;
        compiled_functions.push((Arc::clone(symbol), compiled, false));
    }

    // Collect lambda info: (temp_hash, parent_name, compiled_func).
    let lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)> = ctx.lambdas;
    // Value objects for block-scoped consts, discovered while walking bodies.
    let block_const_objects = ctx.const_objects;

    // Phase 3: Compute content-addressed hashes and finalize the module.
    let mut module = finalize_module_hashes(compiled_functions, lambdas)?;
    // Fold in every const value object — module-level ones from the pre-pass
    // and block-scoped ones from the bodies. They ship and deduplicate
    // alongside function objects; a referencing function already records the
    // const hash in its dependencies.
    for (hash, object) in const_objects.into_iter().chain(block_const_objects) {
        module.objects.entry(hash).or_insert(object);
    }
    module.const_names = const_names;
    Ok(module)
}

/// Compile a function with pre-determined hash.
fn compile_function_with_hash(
    func: &FunctionDef,
    function_hashes: &HashMap<NameKey, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slots for parameters (with names for lookup).
    for param in &func.params {
        fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Compile the function body.
    compile_expr(&mut fc, &func.body, ctx)?;

    // Emit return instruction.
    fc.builder.emit(Opcode::Return);

    let param_count = func.params.len() as u8;

    // Build with the pre-computed dependencies from the builder
    let bytecode = fc.builder.bytecode().to_vec();
    let constants = fc.builder.constants().to_vec();
    let dependencies = fc.builder.dependencies().to_vec();

    // Get the pre-computed hash for this function, keyed like its
    // same-module references (see `own_item_key`).
    let hash = function_hashes[&own_item_key(ctx.module_id.as_ref(), &func.name)];

    // Build debug info if source is available
    let debug_info = if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(func.name.to_string());
        debug_info.source_file = source_file.map(String::from);

        // Compute line/column numbers from source spans
        if let Some(src) = source {
            for mapping in &mut debug_info.source_map {
                let (line, col) = span_to_line_col(
                    src,
                    crate::ast::Span::new(mapping.source_start as u32, mapping.source_end as u32),
                );
                mapping.line = line;
                mapping.column = col;
            }
        }

        Some(debug_info)
    } else {
        None
    };

    Ok(CompiledFunction {
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info,
    })
}

/// Compile a single impl method as an ordinary function.
///
/// The method is registered in the module under its canonical symbol
/// (`types::impl_method_symbol`), so it participates in content-addressed
/// hash finalization exactly like a named function.
fn compile_impl_method(
    impl_def: &ImplDef,
    method: &ImplMethod,
    function_hashes: &HashMap<NameKey, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slot for the self parameter. Associated methods (e.g.
    // `Default::default`) take no `self`, so the slot and its argument are
    // omitted for them.
    if method.has_self {
        let self_name: Arc<str> = "self".into();
        fc.alloc_local_with_name(method.self_id, &self_name)?;
    }

    // Allocate slots for other parameters
    for param in &method.params {
        fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Compile the method body
    compile_expr(&mut fc, &method.body, ctx)?;

    // Emit return instruction
    fc.builder.emit(Opcode::Return);

    // +1 for the self parameter on instance methods; associated methods
    // (no `self`) take only their declared parameters.
    let param_count = (method.params.len() + usize::from(method.has_self)) as u8;

    let bytecode = fc.builder.bytecode().to_vec();
    let constants = fc.builder.constants().to_vec();
    let dependencies = fc.builder.dependencies().to_vec();

    // The temporary hash; finalization replaces it with the content hash.
    let hash = method
        .resolved_symbol
        .as_ref()
        .and_then(|symbol| function_hashes.get(&NameKey::Bare(Arc::clone(symbol))))
        .copied()
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Internal {
                    message: "impl method missing resolved symbol",
                },
                (method.span.start, method.span.end),
            )
        })?;

    // Build debug info if source is available
    let debug_info = if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(match &impl_def.trait_name {
            Some(trait_name) => format!("{}::{}", trait_name.name, method.name),
            None => format!("{}::{}", impl_def.for_type, method.name),
        });
        debug_info.source_file = source_file.map(String::from);

        // Compute line/column numbers from source spans
        if let Some(src) = source {
            for mapping in &mut debug_info.source_map {
                let (line, col) = span_to_line_col(
                    src,
                    crate::ast::Span::new(mapping.source_start as u32, mapping.source_end as u32),
                );
                mapping.line = line;
                mapping.column = col;
            }
        }

        Some(debug_info)
    } else {
        None
    };

    Ok(CompiledFunction {
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
