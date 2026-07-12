//! Per-function and per-module compiler state.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::entry::own_item_key;
use super::error::{CompileError, CompileErrorKind};
use crate::ast::{BindingId, ItemKind, Module};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, DebugInfo, Opcode};
use crate::fqn::{Fqn, ModuleId, NameKey};

/// The unspellable name a hidden dictionary parameter is bound under.
///
/// A bounded item's dictionaries have no source name, but the closure
/// machinery captures free variables *by name*. Binding each dictionary
/// under a stable, index-keyed pseudo-name (the index is the same at every
/// lambda nesting level — it always counts the enclosing bounded item's
/// bounds) lets a dictionary flow through the exact same name-capture path
/// as an ordinary variable, including forwarding through nested lambdas.
/// The `<…>` form can never collide with a parsed identifier.
pub(super) fn dict_capture_name(index: usize) -> Arc<str> {
    Arc::from(format!("<dict#{index}>"))
}

/// Compiler state for a single function.
pub(super) struct FunctionCompiler {
    /// Bytecode builder.
    pub(super) builder: BytecodeBuilder,

    /// Map from binding IDs to local slots.
    pub(super) locals: HashMap<BindingId, u16>,

    /// Map from local variable names to their slots.
    /// This is used when lowering doesn't produce Local(id) references.
    pub(super) local_names: HashMap<Arc<str>, u16>,

    /// Next available local slot.
    pub(super) next_local: u16,

    /// Map from function names to their hashes (for recursive calls).
    pub(super) function_hashes: HashMap<NameKey, blake3::Hash>,

    /// Captured variables (for closures): binding ID -> capture slot index.
    /// These are variables from enclosing scopes that this function captures.
    pub(super) captures: HashMap<BindingId, u16>,

    /// Captured variable names (for closures).
    pub(super) capture_names: HashMap<Arc<str>, u16>,

    /// Parent's locals - used during closure compilation to identify free variables.
    /// Maps binding IDs from the enclosing scope to their local slots there.
    pub(super) parent_locals: Option<HashMap<BindingId, u16>>,

    /// Parent's local names - for name-based lookups during closure compilation.
    pub(super) parent_local_names: Option<HashMap<Arc<str>, u16>>,

    /// Block-scoped `const` names in scope, mapped to their value object's
    /// content hash. A reference emits `LoadObject` of the hash (never a
    /// local slot) — a `const` is a compile-time value, not a runtime local.
    /// Inherited into a nested lambda at creation, since the hash is
    /// position-independent and an enclosing const is visible in the closure.
    pub(super) block_consts: HashMap<Arc<str>, blake3::Hash>,

    /// Local slots of this function's hidden dictionary parameters, in
    /// declaration order (one per entry of [`crate::ast::dict_params`]).
    /// `ResolvedMethod::DictSlot` and `DictSource::Param` index into this.
    pub(super) dict_locals: Vec<u16>,

    /// Debug information being built.
    pub(super) debug_info: DebugInfo,
}

impl FunctionCompiler {
    /// Create a new function compiler.
    pub(super) fn new(function_hashes: HashMap<NameKey, blake3::Hash>) -> Self {
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
            dict_locals: Vec::new(),
            debug_info: DebugInfo::new(),
        }
    }

    /// Create a function compiler for a closure nested directly inside
    /// `parent`'s scope.
    ///
    /// A closure may reference any binding its enclosing scopes can supply,
    /// not just the immediate parent's own locals: the name-capture view
    /// unions the parent's locals, the parent's own captures, and everything
    /// the parent could itself capture (its parent view). That transitive
    /// union is what lets a free variable — or a hidden dictionary — thread
    /// through several nested lambdas: each level forwards it as an ordinary
    /// capture (see `emit_captured_or_local`). The by-id (`parent_locals`)
    /// channel stays one level deep, as it only serves hand-built ASTs that
    /// never nest-capture.
    pub(super) fn for_closure(parent: &FunctionCompiler) -> Self {
        let mut parent_local_names = parent.local_names.clone();
        for (name, slot) in &parent.capture_names {
            parent_local_names.entry(Arc::clone(name)).or_insert(*slot);
        }
        if let Some(grand) = &parent.parent_local_names {
            for (name, slot) in grand {
                parent_local_names.entry(Arc::clone(name)).or_insert(*slot);
            }
        }
        Self {
            builder: BytecodeBuilder::new(),
            locals: HashMap::new(),
            local_names: HashMap::new(),
            next_local: 0,
            function_hashes: parent.function_hashes.clone(),
            captures: HashMap::new(),
            capture_names: HashMap::new(),
            parent_locals: Some(parent.locals.clone()),
            parent_local_names: Some(parent_local_names),
            // A lambda inherits the enclosing block consts in scope; each is a
            // bare `LoadObject` of a hash, so no capture slot is needed.
            block_consts: parent.block_consts.clone(),
            dict_locals: Vec::new(),
            debug_info: DebugInfo::new(),
        }
    }

    /// Record a source mapping for the current bytecode position.
    ///
    /// This associates the current bytecode offset with the given source span.
    /// Line and column are set to 0 initially; they can be computed later when
    /// the source code is available.
    pub(super) fn record_span(&mut self, span: crate::ast::Span) {
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
    pub(super) fn record_local_name(&mut self, slot: u16, name: &str) {
        self.debug_info.add_local_name(slot, name);
    }

    /// Allocate a local slot for a binding with a name.
    pub(super) fn alloc_local_with_name(
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
    pub(super) fn get_local(&self, id: BindingId, span: (u32, u32)) -> Result<u16, CompileError> {
        self.locals
            .get(&id)
            .copied()
            .ok_or_else(|| CompileError::new(CompileErrorKind::UndefinedLocal { id }, span))
    }

    /// Get the local slot for a binding by name.
    pub(super) fn get_local_by_name(&self, name: &str) -> Option<u16> {
        self.local_names.get(name).copied()
    }

    /// Check if a binding ID is from the parent scope (needs to be captured).
    pub(super) fn is_parent_binding(&self, id: BindingId) -> bool {
        if let Some(parent) = &self.parent_locals {
            parent.contains_key(&id) && !self.locals.contains_key(&id)
        } else {
            false
        }
    }

    /// Check if a name is from the parent scope (needs to be captured).
    pub(super) fn is_parent_name(&self, name: &str) -> bool {
        if let Some(parent) = &self.parent_local_names {
            parent.contains_key(name) && !self.local_names.contains_key(name)
        } else {
            false
        }
    }

    /// Get or create a capture slot for a parent binding.
    pub(super) fn get_or_create_capture(&mut self, id: BindingId, name: Arc<str>) -> u16 {
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
    pub(super) fn get_or_create_capture_by_name(&mut self, name: Arc<str>) -> u16 {
        if let Some(&slot) = self.capture_names.get(&name) {
            slot
        } else {
            // Use capture_names.len() since we're tracking by name
            let slot = self.capture_names.len() as u16;
            self.capture_names.insert(name, slot);
            slot
        }
    }

    /// Emit code that pushes the value bound to `name` onto the stack: from
    /// this function's own locals, from a capture it already holds, or — when
    /// it is a closure and `name` is free — by capturing it from the
    /// enclosing scope (which, transitively, forwards it down the nesting
    /// chain). This is the single place free-variable and dictionary loads
    /// resolve, so every closure form (lambdas, handler arms, handle thunks)
    /// captures identically. An unknown name is a compiler bug — the checker
    /// only leaves references it resolved.
    pub(super) fn emit_captured_or_local(
        &mut self,
        name: &Arc<str>,
        span: (u32, u32),
    ) -> Result<(), CompileError> {
        if let Some(&slot) = self.local_names.get(name) {
            self.builder.emit_u16(Opcode::LoadLocal, slot);
        } else if let Some(&capture_slot) = self.capture_names.get(name) {
            self.builder.emit_load_capture(capture_slot);
        } else if self.is_parent_name(name) {
            let capture_slot = self.get_or_create_capture_by_name(Arc::clone(name));
            self.builder.emit_load_capture(capture_slot);
        } else {
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown capture: {name}"),
                },
                span,
            ));
        }
        Ok(())
    }

    /// Push this function's `dict_index`-th hidden dictionary onto the stack.
    ///
    /// In the bounded item itself the dictionary is an ordinary local; inside
    /// a lambda in its body it is a captured value. Routing through
    /// [`Self::emit_captured_or_local`] under the dictionary's pseudo-name
    /// (see [`dict_capture_name`]) means both cases — and nested lambdas —
    /// are handled by the one capture path.
    pub(super) fn emit_load_dict(
        &mut self,
        dict_index: usize,
        span: (u32, u32),
    ) -> Result<(), CompileError> {
        self.emit_captured_or_local(&dict_capture_name(dict_index), span)
    }

    /// Get the list of captured names in capture slot order.
    pub(super) fn get_capture_names_in_order(&self) -> Vec<(Arc<str>, u16)> {
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
pub(super) struct ModuleContext {
    /// Lambda functions discovered during compilation.
    /// Maps (temporary hash, parent function name) to compiled function.
    pub(super) lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
    /// Statically-named `State::init_versioned` obligations discovered
    /// while compiling bodies (see [`super::MigrationRecord`]); folded
    /// into [`super::CompiledModule::migrations`] after finalization.
    pub(super) migrations: Vec<super::MigrationRecord>,
    /// Counter for generating unique lambda temporary hashes.
    pub(super) lambda_counter: u32,
    /// Value objects for block-scoped `const`s discovered while compiling
    /// function bodies. Folded into the module's objects after finalization
    /// (deduplicated by hash), alongside module-level const objects. Module
    /// consts are content-addressed in a pre-pass instead; these can only be
    /// found by walking bodies.
    pub(super) const_objects: HashMap<blake3::Hash, crate::object::StoredObject>,
    /// Name of the function currently being compiled.
    /// Used to track lambda parent relationships.
    pub(super) current_function: Option<Arc<str>>,
    /// Enum variant constructors in scope: variant name → variant info.
    /// Seeded with the prelude (Option/Result); local enum declarations
    /// shadow prelude variants of the same name.
    pub(super) enums: HashMap<Arc<str>, VariantInfo>,
    /// Foreign enum variant constructors keyed by their canonical
    /// two-segment [`Fqn`] (`core::option::Some`,
    /// `pkg::shapes::Shape::Circle`). Consulted *before* the bare
    /// [`Self::enums`] table so a fully-qualified reference always inlines
    /// the defining enum's tag, never a same-named local variant's (see
    /// `CompileOptions::foreign_enum_variants`).
    pub(super) foreign_variants: HashMap<Fqn, VariantInfo>,
    /// Unit-struct constructors in scope, keyed by resolution key: local
    /// declarations by bare name ([`NameKey::Bare`]), foreign ones by their
    /// [`Fqn`] ([`NameKey::Item`]). A reference whose key is here compiles
    /// to an empty record value, mirroring a nullary enum variant.
    pub(super) unit_structs: HashSet<NameKey>,
    /// Module-level constants: resolution key → the content hash of the
    /// `const`'s value object. A reference to one compiles to a `LoadObject`
    /// of this hash, so a constant links through the same name→hash table as
    /// a function — deduplicated and hash-addressed, not inlined. Local
    /// constants key on their `Fqn` (bare in the registry-less convention),
    /// imported ones by their `Fqn`; both hashes are final (value objects are
    /// leaves computed in a pre-pass). Key present ⇒ the name is a const.
    pub(super) const_hashes: HashMap<NameKey, blake3::Hash>,
    /// Module-declared abilities in scope: ability name → compile info.
    /// The identity comes from the type checker (`AbilityDef::resolved_id`);
    /// the compiler never re-derives these identities. Local declarations
    /// resolve bare, so this stays a bare-name table.
    pub(super) abilities: HashMap<Arc<str>, CompiledAbilityInfo>,
    /// Foreign ability identities keyed by their [`Fqn`]; see
    /// [`CompileOptions::foreign_abilities`].
    pub(super) foreign_abilities: HashMap<Fqn, CompiledAbilityInfo>,
    /// The current module's identity (registry-less compiles pass `None`).
    /// The module's own consts and unit structs key on their `Fqn` under
    /// this id, matching the resolve pass; a same-module ability reference
    /// arrives as `Item(Fqn)` and resolves back to the bare-keyed local
    /// ability table via this id.
    pub(super) module_id: Option<ModuleId>,
}

/// Compile-time info for one module-declared ability.
#[derive(Debug, Clone)]
pub(crate) struct CompiledAbilityInfo {
    /// The uuid-derived identity.
    pub id: crate::types::AbilityId,
    /// The declaration uuid (a `MethodKey` input, and the root of every
    /// method's impl-dispatch symbol `<uuid>::<method>`).
    pub uuid: uuid::Uuid,
    /// Methods in declaration order.
    pub methods: Vec<CompiledMethodInfo>,
}

/// Compile-time info for one ability method.
#[derive(Debug, Clone)]
pub(crate) struct CompiledMethodInfo {
    pub name: Arc<str>,
    /// Canonical signature hash (a `MethodKey` input), from the checker.
    pub signature: ambient_core::SignatureHash,
    /// Whether a default implementation exists (`false` only for abstract
    /// never-returning methods, e.g. `Exception::throw`).
    pub has_impl: bool,
    /// Whether the method returns `!` (never) — its performs unwind
    /// instead of capturing a continuation.
    pub never: bool,
}

impl CompiledAbilityInfo {
    /// Look up a method by name.
    pub(crate) fn method(&self, name: &str) -> Option<&CompiledMethodInfo> {
        self.methods.iter().find(|m| m.name.as_ref() == name)
    }

    /// The dispatch symbol a method's default implementation compiles
    /// under: `<uuid>::<method>` — a content symbol like impl methods'
    /// `<type-uuid>::<method>`, so it links by [`NameKey::Bare`]
    /// everywhere and never collides with module-path names.
    pub(crate) fn impl_symbol(&self, method: &str) -> Arc<str> {
        Arc::from(format!("{}::{}", self.uuid, method))
    }
}

/// Compile-time info for one enum variant constructor. Defined with the
/// rest of the cross-module channels in [`crate::module_env`]; re-exported
/// here because the compiler is its primary consumer.
pub use crate::module_env::VariantInfo;

/// The compiler's view of a checker-resolved ability.
fn compiled_info(ability: &crate::ability_resolver::DynAbility) -> CompiledAbilityInfo {
    CompiledAbilityInfo {
        id: ability.id,
        uuid: ability.uuid,
        methods: ability
            .methods
            .iter()
            .map(|m| CompiledMethodInfo {
                name: Arc::clone(&m.name),
                signature: m.signature,
                has_impl: m.has_impl,
                never: matches!(m.ret, crate::types::Type::Never),
            })
            .collect(),
    }
}

impl ModuleContext {
    pub(super) fn new(module_id: Option<ModuleId>) -> Self {
        // Option/Result carry no hardcoded seed: they arrive through the same
        // `imported_enums` channel as any other enum, folded in from the
        // prelude by `ModuleEnv::new`. A registry-less compile (no prelude)
        // therefore starts with no enums, exactly like the checker.
        Self {
            lambdas: Vec::new(),
            migrations: Vec::new(),
            lambda_counter: 0,
            const_objects: HashMap::new(),
            current_function: None,
            enums: HashMap::new(),
            foreign_variants: HashMap::new(),
            unit_structs: HashSet::new(),
            const_hashes: HashMap::new(),
            abilities: HashMap::new(),
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
    pub(super) fn register_const_hashes(&mut self, hashes: &HashMap<NameKey, blake3::Hash>) {
        for (key, hash) in hashes {
            self.const_hashes.insert(key.clone(), *hash);
        }
    }

    /// The content hash of the module-level constant a reference resolves to,
    /// if any. A present key means the name denotes a `const`.
    pub(super) fn constant_hash(&self, key: &NameKey) -> Option<blake3::Hash> {
        self.const_hashes.get(key).copied()
    }

    /// Resolve an ability reference to its compile-time info.
    ///
    /// A resolved reference names a same-module ability (its `Fqn`'s
    /// module is ours — look it up bare in the local table) or a foreign
    /// ability by its `Fqn` (`core::system` included: the platform module
    /// compiles like any other, so its abilities arrive through the
    /// ordinary foreign channel). A bare unresolved reference is a local
    /// declaration — same-module abilities are never resolved to an `Fqn`.
    pub(super) fn resolve_ability(
        &self,
        ability: &crate::ast::QualifiedName,
    ) -> Option<&CompiledAbilityInfo> {
        match &ability.resolved {
            Some(fqn) => self
                .module_id
                .as_ref()
                .filter(|id| **id == fqn.module)
                .and_then(|_| self.abilities.get(fqn.name()))
                .or_else(|| self.foreign_abilities.get(fqn)),
            None => self.abilities.get(&ability.name),
        }
    }

    /// Register the foreign-ability channel (see
    /// [`CompileOptions::foreign_abilities`]).
    pub(super) fn register_foreign_abilities(
        &mut self,
        foreign: &[(Fqn, std::sync::Arc<crate::ability_resolver::DynAbility>)],
    ) {
        for (fqn, dyn_ab) in foreign {
            self.foreign_abilities
                .insert(fqn.clone(), compiled_info(dyn_ab));
        }
    }

    /// Register a module's `ability` declarations from their type-checked
    /// identities.
    pub(super) fn register_abilities(&mut self, module: &Module) -> Result<(), CompileError> {
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
                let methods = def
                    .methods
                    .iter()
                    .map(|m| {
                        let Some(signature) = m.resolved_signature else {
                            return Err(CompileError::new(
                                CompileErrorKind::Internal {
                                    message: "ability method missing resolved signature",
                                },
                                (m.span.start, m.span.end),
                            ));
                        };
                        Ok(CompiledMethodInfo {
                            name: Arc::clone(&m.name),
                            signature,
                            has_impl: m.body.is_some(),
                            never: matches!(m.ret_ty, crate::types::Type::Never),
                        })
                    })
                    .collect::<Result<Vec<_>, CompileError>>()?;
                self.abilities.insert(
                    Arc::clone(&def.name),
                    CompiledAbilityInfo {
                        id,
                        uuid: def.uuid,
                        methods,
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
    pub(super) fn ability_by_id(
        &self,
        id: crate::types::AbilityId,
    ) -> Option<(&Arc<str>, &CompiledAbilityInfo)> {
        self.abilities
            .iter()
            .find(|(_, info)| info.id == id)
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
    pub(super) fn register_enum_def(&mut self, enum_def: &crate::ast::EnumDef) {
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
    pub(super) fn register_imported_enums(&mut self, imported: &[crate::ast::EnumDef]) {
        for enum_def in imported {
            self.register_enum_def(enum_def);
        }
    }

    /// Register foreign enum variant constructors under their canonical
    /// two-segment [`Fqn`] (see [`CompileOptions::foreign_enum_variants`]).
    /// A separate table from [`Self::enums`]: it is keyed by `Fqn`, not
    /// bare name, so a qualified reference is never shadowed by (nor
    /// shadows) a same-named local variant.
    pub(super) fn register_foreign_variants(&mut self, variants: &[(Fqn, VariantInfo)]) {
        for (fqn, info) in variants {
            self.foreign_variants.insert(fqn.clone(), info.clone());
        }
    }

    /// Register a module's enum declarations, shadowing prelude variants.
    pub(super) fn register_enums(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Enum(enum_def) = &item.kind {
                self.register_enum_def(enum_def);
            }
        }
    }

    /// Register the local module's unit structs under their `Fqn` (or bare,
    /// registry-less) — their resolution key when referenced from within
    /// the module.
    pub(super) fn register_unit_structs(&mut self, module: &Module) {
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
    pub(super) fn register_imported_unit_structs(&mut self, keys: &[NameKey]) {
        for key in keys {
            self.unit_structs.insert(key.clone());
        }
    }

    /// Set the current function being compiled.
    pub(super) fn set_current_function(&mut self, name: Arc<str>) {
        self.current_function = Some(name);
    }

    /// Generate a unique temporary hash for a lambda.
    pub(super) fn next_lambda_hash(&mut self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"__lambda__");
        hasher.update(&self.lambda_counter.to_le_bytes());
        self.lambda_counter += 1;
        hasher.finalize()
    }

    /// Register a compiled lambda and return its temporary hash.
    /// The lambda is associated with the current function being compiled.
    pub(super) fn register_lambda(&mut self, function: CompiledFunction) -> blake3::Hash {
        let hash = self.next_lambda_hash();
        let parent = self
            .current_function
            .clone()
            .unwrap_or_else(|| Arc::from("__unknown__"));
        self.lambdas.push((hash, parent, function));
        hash
    }
}
