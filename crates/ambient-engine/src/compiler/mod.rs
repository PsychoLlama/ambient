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
//! - Main module - Expression/statement compilation, module compilation

mod error;
mod intrinsics;
mod lambdas;
mod patterns;
mod repl;

pub use error::{CompileError, CompileErrorKind};
pub use repl::{
    compile_expression, compile_expression_with_context, compile_repl_item, parse_module_exports,
    CompiledReplItem, ReplContext, ReplItemKind,
};

use intrinsics::try_compile_intrinsic;
use lambdas::{compile_ability_call, compile_handle_expr, compile_handler_literal, compile_lambda};
use patterns::compile_match;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{
    BinaryOp, BindingId, ConstDef, Expr, ExprKind, FunctionDef, ItemKind, LetBinding, Module, Stmt,
    StmtKind, UnaryOp,
};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, DebugInfo, Opcode};
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
    pub function_names: HashMap<Arc<str>, blake3::Hash>,

    /// The entry point function (typically "run").
    pub entry_point: Option<blake3::Hash>,
}

impl CompiledModule {
    /// Create an empty compiled module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            function_names: HashMap::new(),
            entry_point: None,
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
        // Don't overwrite entry point if we already have one
        if self.entry_point.is_none() {
            self.entry_point = other.entry_point;
        }
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
    function_hashes: HashMap<Arc<str>, blake3::Hash>,

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

    /// Debug information being built.
    debug_info: DebugInfo,

    /// REPL context for identifying constants (which need to be auto-called).
    repl_context: Option<ReplContext>,
}

impl FunctionCompiler {
    /// Create a new function compiler.
    fn new(function_hashes: HashMap<Arc<str>, blake3::Hash>) -> Self {
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
            debug_info: DebugInfo::new(),
            repl_context: None,
        }
    }

    /// Set the REPL context for this compiler.
    fn set_repl_context(&mut self, context: &ReplContext) {
        self.repl_context = Some(context.clone());
    }

    /// Check if a name refers to a constant (in REPL mode).
    fn is_repl_constant(&self, name: &str) -> bool {
        self.repl_context
            .as_ref()
            .is_some_and(|ctx| ctx.is_constant(name))
    }

    /// Create a function compiler for a closure, with access to parent scope.
    fn new_for_closure(
        function_hashes: HashMap<Arc<str>, blake3::Hash>,
        parent_locals: HashMap<BindingId, u16>,
        parent_local_names: HashMap<Arc<str>, u16>,
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
            debug_info: DebugInfo::new(),
            repl_context: None,
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
    /// Maps temporary hash to compiled function.
    lambdas: Vec<(blake3::Hash, CompiledFunction)>,
    /// Counter for generating unique lambda names.
    lambda_counter: u32,
}

impl ModuleContext {
    fn new() -> Self {
        Self {
            lambdas: Vec::new(),
            lambda_counter: 0,
        }
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
    fn register_lambda(&mut self, function: CompiledFunction) -> blake3::Hash {
        let hash = self.next_lambda_hash();
        self.lambdas.push((hash, function));
        hash
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Module Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile a module to bytecode.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module(module: &Module) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, None, None, None)
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
    imported_hashes: HashMap<Arc<str>, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, None, None, Some(imported_hashes))
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
    compile_module_impl(module, Some(source), Some(source_file), None)
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
    imported_hashes: HashMap<Arc<str>, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        Some(source),
        Some(source_file),
        Some(imported_hashes),
    )
}

/// Implementation of module compilation with optional debug info.
fn compile_module_impl(
    module: &Module,
    source: Option<&str>,
    source_file: Option<&str>,
    imported_hashes: Option<HashMap<Arc<str>, blake3::Hash>>,
) -> Result<CompiledModule, CompileError> {
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
    let mut temp_hashes: HashMap<Arc<str>, blake3::Hash> = imported_hashes.unwrap_or_default();

    // Add temporary hashes for local functions.
    for func in &functions {
        let hash = compute_temporary_hash(&func.name);
        temp_hashes.insert(Arc::clone(&func.name), hash);
    }

    // Create module context for tracking lambdas discovered during compilation.
    let mut ctx = ModuleContext::new();

    // Phase 2: Compile each function using temporary hashes.
    let mut compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)> = Vec::new();
    for func in &functions {
        let compiled =
            compile_function_with_hash(func, &temp_hashes, &mut ctx, source, source_file)?;
        let is_main = &*func.name == "run";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Compile constants.
    for item in &module.items {
        if let ItemKind::Const(const_def) = &item.kind {
            let compiled = compile_const(const_def, &temp_hashes, &mut ctx, source, source_file)?;
            compiled_functions.push((Arc::clone(&const_def.name), compiled, false));
        }
    }

    // Add lambda functions discovered during compilation.
    // Generate synthetic names for them in temp_hashes.
    for (lambda_hash, compiled_func) in ctx.lambdas {
        let lambda_name: Arc<str> = format!("__lambda_{lambda_hash}").into();
        temp_hashes.insert(Arc::clone(&lambda_name), lambda_hash);
        compiled_functions.push((lambda_name, compiled_func, false));
    }

    // Phase 3: Compute content-addressed hashes and finalize the module.
    finalize_module_hashes(compiled_functions, &temp_hashes)
}

/// Finalize content-addressed hashes for all compiled functions.
///
/// This handles:
/// 1. Non-recursive functions: compute hash from bytecode content
/// 2. Recursive functions (SCCs): compute group hash for mutual recursion
#[allow(clippy::too_many_lines)]
fn finalize_module_hashes(
    compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    // Build reverse mapping: temp_hash -> name
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    // Build call graph: for each function, which other functions does it call?
    // We detect this by looking at FunctionRef values in the constant pool.
    let mut call_graph: HashMap<Arc<str>, Vec<Arc<str>>> = HashMap::new();
    for (name, func, _) in &compiled_functions {
        let mut calls = Vec::new();
        for constant in &func.constants {
            if let Value::FunctionRef(hash) = constant {
                if let Some(called_name) = temp_to_name.get(hash) {
                    calls.push(Arc::clone(called_name));
                }
            }
        }
        call_graph.insert(Arc::clone(name), calls);
    }

    // Find SCCs using Tarjan's algorithm (generic implementation from store module)
    let scc_analysis = compute_sccs(&call_graph);

    // Compute final hashes in topological order (dependencies before dependents)
    let mut final_hashes: HashMap<Arc<str>, blake3::Hash> = HashMap::new();

    // Pre-populate with imported function hashes.
    // Imported functions are already content-addressed, so we can use them directly.
    // They're in temp_hashes but not in compiled_functions.
    let local_names: HashSet<&Arc<str>> = compiled_functions.iter().map(|(n, _, _)| n).collect();
    for (name, hash) in temp_hashes {
        if !local_names.contains(name) {
            // This is an imported function - use its hash directly
            final_hashes.insert(Arc::clone(name), *hash);
        }
    }

    for scc in &scc_analysis.components {
        if scc.is_singleton() {
            // Single function - might be self-recursive or not
            let name = &scc.members[0];

            // Skip if this is an imported function (already in final_hashes)
            if final_hashes.contains_key(name) {
                continue;
            }

            let func = compiled_functions
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, f, _)| f)
                .ok_or_else(|| {
                    CompileError::new(
                        CompileErrorKind::Internal {
                            message: "function should exist in compiled_functions",
                        },
                        (0, 0),
                    )
                })?;

            // Check if it's self-recursive
            let is_self_recursive = call_graph
                .get(name)
                .is_some_and(|calls| calls.contains(name));

            if is_self_recursive {
                // Self-recursive: compute hash excluding self-reference
                let hash = compute_scc_hash(
                    &scc.members,
                    &compiled_functions,
                    &final_hashes,
                    temp_hashes,
                );
                final_hashes.insert(Arc::clone(name), hash);
            } else {
                // Non-recursive: compute hash with resolved dependencies
                let hash = compute_content_hash(func, &final_hashes, temp_hashes);
                final_hashes.insert(Arc::clone(name), hash);
            }
        } else {
            // Multiple functions in SCC - mutual recursion
            // Filter out imported functions (already in final_hashes)
            let local_members: Vec<&Arc<str>> = scc
                .members
                .iter()
                .filter(|name| !final_hashes.contains_key(*name))
                .collect();

            if local_members.is_empty() {
                // All members are imported - skip
                continue;
            }

            // Compute a group hash for the entire SCC
            let scc_hash = compute_scc_hash(
                &scc.members,
                &compiled_functions,
                &final_hashes,
                temp_hashes,
            );

            // Each local function in the SCC gets a derived hash
            for (idx, name) in local_members.iter().enumerate() {
                let mut hasher = blake3::Hasher::new();
                hasher.update(scc_hash.as_bytes());
                hasher.update(&(idx as u32).to_le_bytes());
                let hash = hasher.finalize();
                final_hashes.insert(Arc::clone(name), hash);
            }
        }
    }

    // Phase 4: Update all functions with final hashes
    let mut result = CompiledModule::new();

    for (name, mut func, is_main) in compiled_functions {
        // Update FunctionRef values in constant pool
        for constant in &mut func.constants {
            if let Value::FunctionRef(ref mut hash) = constant {
                if let Some(called_name) = temp_to_name.get(hash) {
                    if let Some(&final_hash) = final_hashes.get(called_name) {
                        *hash = final_hash;
                    }
                }
            }
        }

        // Update dependencies
        func.dependencies = func
            .dependencies
            .iter()
            .filter_map(|dep| {
                temp_to_name
                    .get(dep)
                    .and_then(|name| final_hashes.get(name))
                    .copied()
            })
            .collect();

        // Get the final hash for this function
        let final_hash = final_hashes.get(&name).copied().ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Internal {
                    message: "all functions should have final hashes",
                },
                (0, 0),
            )
        })?;

        // Update the function's hash field
        func.hash = final_hash;

        result.functions.insert(final_hash, func);
        result.function_names.insert(name, final_hash);

        if is_main {
            result.entry_point = Some(final_hash);
        }
    }

    Ok(result)
}

/// Compute content-addressed hash for a non-recursive function.
fn compute_content_hash(
    func: &CompiledFunction,
    final_hashes: &HashMap<Arc<str>, blake3::Hash>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> blake3::Hash {
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    let mut hasher = blake3::Hasher::new();

    // Hash bytecode
    hasher.update(&(func.bytecode.len() as u32).to_le_bytes());
    hasher.update(&func.bytecode);

    // Hash constants with resolved function references
    hasher.update(&(func.constants.len() as u32).to_le_bytes());
    for constant in &func.constants {
        match constant {
            Value::FunctionRef(hash) => {
                // Resolve to final hash if available
                let resolved = temp_to_name
                    .get(hash)
                    .and_then(|name| final_hashes.get(name))
                    .copied()
                    .unwrap_or(*hash);
                hasher.update(&[6u8]); // TYPE_FUNCTION_REF
                hasher.update(resolved.as_bytes());
            }
            _ => hash_value_for_content(&mut hasher, constant),
        }
    }

    // Hash metadata
    hasher.update(&func.local_count.to_le_bytes());
    hasher.update(&[func.param_count]);

    // Hash resolved dependencies
    let resolved_deps: Vec<blake3::Hash> = func
        .dependencies
        .iter()
        .filter_map(|dep| {
            temp_to_name
                .get(dep)
                .and_then(|name| final_hashes.get(name))
                .copied()
        })
        .collect();

    hasher.update(&(resolved_deps.len() as u32).to_le_bytes());
    for dep in &resolved_deps {
        hasher.update(dep.as_bytes());
    }

    hasher.finalize()
}

/// Compute a combined hash for a strongly connected component (recursive functions).
fn compute_scc_hash(
    scc: &[Arc<str>],
    compiled_functions: &[(Arc<str>, CompiledFunction, bool)],
    final_hashes: &HashMap<Arc<str>, blake3::Hash>,
    temp_hashes: &HashMap<Arc<str>, blake3::Hash>,
) -> blake3::Hash {
    let temp_to_name: HashMap<blake3::Hash, Arc<str>> = temp_hashes
        .iter()
        .map(|(name, hash)| (*hash, Arc::clone(name)))
        .collect();

    // Create a set of names in this SCC for quick lookup
    let scc_set: std::collections::HashSet<&Arc<str>> = scc.iter().collect();

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"__scc__");
    hasher.update(&(scc.len() as u32).to_le_bytes());

    // Sort SCC members for deterministic ordering
    let mut sorted_scc: Vec<_> = scc.to_vec();
    sorted_scc.sort();

    for name in &sorted_scc {
        // SAFETY: All functions in the SCC must exist in compiled_functions
        #[allow(clippy::expect_used)]
        let func = compiled_functions
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, f, _)| f)
            .expect("function should exist");

        // Hash the function name (for position in SCC)
        hasher.update(&(name.len() as u32).to_le_bytes());
        hasher.update(name.as_bytes());

        // Hash bytecode
        hasher.update(&(func.bytecode.len() as u32).to_le_bytes());
        hasher.update(&func.bytecode);

        // Hash constants, but use placeholders for SCC-internal references
        hasher.update(&(func.constants.len() as u32).to_le_bytes());
        for constant in &func.constants {
            match constant {
                Value::FunctionRef(hash) => {
                    if let Some(called_name) = temp_to_name.get(hash) {
                        if scc_set.contains(called_name) {
                            // Internal SCC reference - use canonical placeholder
                            hasher.update(&[6u8]); // TYPE_FUNCTION_REF
                            hasher.update(b"__scc_internal__");
                            hasher.update(called_name.as_bytes());
                        } else if let Some(&final_hash) = final_hashes.get(called_name) {
                            // External reference - use final hash
                            hasher.update(&[6u8]);
                            hasher.update(final_hash.as_bytes());
                        } else {
                            // Unknown reference - use temp hash
                            hasher.update(&[6u8]);
                            hasher.update(hash.as_bytes());
                        }
                    } else {
                        hasher.update(&[6u8]);
                        hasher.update(hash.as_bytes());
                    }
                }
                _ => hash_value_for_content(&mut hasher, constant),
            }
        }

        // Hash metadata
        hasher.update(&func.local_count.to_le_bytes());
        hasher.update(&[func.param_count]);
    }

    hasher.finalize()
}

/// Hash a value for content-addressing (mirrors bytecode.rs but accessible here).
#[allow(clippy::too_many_lines)]
fn hash_value_for_content(hasher: &mut blake3::Hasher, value: &Value) {
    const TYPE_UNIT: u8 = 0;
    const TYPE_BOOL: u8 = 1;
    const TYPE_NUMBER: u8 = 2;
    const TYPE_STRING: u8 = 3;
    const TYPE_TUPLE: u8 = 4;
    const TYPE_RECORD: u8 = 5;
    const TYPE_FUNCTION_REF: u8 = 6;
    const TYPE_SUSPENDED_ABILITY: u8 = 7;
    const TYPE_CONTINUATION: u8 = 8;

    match value {
        Value::Unit => {
            hasher.update(&[TYPE_UNIT]);
        }
        Value::Bool(b) => {
            hasher.update(&[TYPE_BOOL, u8::from(*b)]);
        }
        Value::Number(n) => {
            hasher.update(&[TYPE_NUMBER]);
            hasher.update(&n.to_bits().to_le_bytes());
        }
        Value::String(s) => {
            hasher.update(&[TYPE_STRING]);
            hasher.update(&(s.len() as u32).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        Value::Tuple(elements) => {
            hasher.update(&[TYPE_TUPLE]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Record(fields) => {
            hasher.update(&[TYPE_RECORD]);
            let mut sorted_fields: Vec<_> = fields.iter().collect();
            sorted_fields.sort_by(|a, b| a.0.cmp(b.0));
            hasher.update(&(sorted_fields.len() as u32).to_le_bytes());
            for (key, val) in sorted_fields {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
                hash_value_for_content(hasher, val);
            }
        }
        Value::FunctionRef(h) => {
            hasher.update(&[TYPE_FUNCTION_REF]);
            hasher.update(h.as_bytes());
        }
        Value::SuspendedAbility(ability) => {
            hasher.update(&[TYPE_SUSPENDED_ABILITY]);
            hasher.update(&ability.ability_id.to_le_bytes());
            hasher.update(&ability.method_id.to_le_bytes());
            hasher.update(&(ability.args.len() as u32).to_le_bytes());
            for arg in &ability.args {
                hash_value_for_content(hasher, arg);
            }
        }
        Value::Continuation(_) => {
            hasher.update(&[TYPE_CONTINUATION]);
        }
        Value::Closure(closure) => {
            const TYPE_CLOSURE: u8 = 9;
            hasher.update(&[TYPE_CLOSURE]);
            hasher.update(closure.function_hash.as_bytes());
            hasher.update(&(closure.environment.len() as u32).to_le_bytes());
            for val in &closure.environment {
                hash_value_for_content(hasher, val);
            }
        }
        Value::Handler(handler) => {
            const TYPE_HANDLER: u8 = 10;
            hasher.update(&[TYPE_HANDLER]);
            hasher.update(&handler.ability_id.to_le_bytes());
            // Hash methods in sorted order for deterministic hashing
            let mut methods: Vec<_> = handler.methods.iter().collect();
            methods.sort_by_key(|(k, _)| *k);
            hasher.update(&(methods.len() as u32).to_le_bytes());
            for (method_id, func_hash) in methods {
                hasher.update(&method_id.to_le_bytes());
                hasher.update(func_hash.as_bytes());
            }
            // Hash captures
            hasher.update(&(handler.captures.len() as u32).to_le_bytes());
            for val in &handler.captures {
                hash_value_for_content(hasher, val);
            }
        }
        Value::List(elements) => {
            const TYPE_LIST: u8 = 11;
            hasher.update(&[TYPE_LIST]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Map(map) => {
            const TYPE_MAP: u8 = 12;
            hasher.update(&[TYPE_MAP]);
            // BTreeMap is already sorted, so iteration order is deterministic
            hasher.update(&(map.entries.len() as u32).to_le_bytes());
            for (key, val) in &map.entries {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
                hash_value_for_content(hasher, val);
            }
        }
        Value::Set(set) => {
            const TYPE_SET: u8 = 13;
            hasher.update(&[TYPE_SET]);
            hasher.update(&(set.elements.len() as u32).to_le_bytes());
            for elem in &set.elements {
                hash_value_for_content(hasher, elem);
            }
        }
        Value::Enum(e) => {
            const TYPE_ENUM: u8 = 14;
            hasher.update(&[TYPE_ENUM]);
            // Hash type name
            hasher.update(&(e.type_name.len() as u32).to_le_bytes());
            hasher.update(e.type_name.as_bytes());
            // Hash tag
            hasher.update(&e.tag.to_le_bytes());
            // Hash variant name
            hasher.update(&(e.variant_name.len() as u32).to_le_bytes());
            hasher.update(e.variant_name.as_bytes());
            // Hash payload (if any)
            if let Some(payload) = e.payload.as_deref() {
                hasher.update(&[1u8]); // has payload marker
                hash_value_for_content(hasher, payload);
            } else {
                hasher.update(&[0u8]); // no payload marker
            }
        }
        Value::Module(m) => {
            const TYPE_MODULE: u8 = 15;
            hasher.update(&[TYPE_MODULE]);
            hasher.update(&(m.path.len() as u32).to_le_bytes());
            hasher.update(m.path.as_bytes());
        }
    }
}

// Use the generic SCC implementation from store module
use crate::store::compute_sccs;

/// Compute a temporary hash for a function based on its name.
/// This is only used during the initial compilation pass; the final
/// content-addressed hash is computed after all functions are compiled.
fn compute_temporary_hash(name: &str) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"__temp_hash__");
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// Compile a function with pre-determined hash.
fn compile_function_with_hash(
    func: &FunctionDef,
    function_hashes: &HashMap<Arc<str>, blake3::Hash>,
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

    // Get the pre-computed hash for this function
    let hash = function_hashes[&func.name];

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

/// Compile a constant definition to a function that returns the constant value.
fn compile_const(
    const_def: &ConstDef,
    function_hashes: &HashMap<Arc<str>, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Compile the value expression.
    compile_expr(&mut fc, &const_def.value, ctx)?;

    // Return the value.
    fc.builder.emit(Opcode::Return);

    let mut compiled = fc.builder.build(fc.next_local, 0);

    // Build debug info if source is available
    if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(const_def.name.to_string());
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

        compiled.debug_info = Some(debug_info);
    }

    Ok(compiled)
}

// ─────────────────────────────────────────────────────────────────────────────
// Expression Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile an expression, pushing its value onto the stack.
#[allow(clippy::too_many_lines)]
fn compile_expr(
    fc: &mut FunctionCompiler,
    expr: &Expr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Record source location for debugging
    fc.record_span(expr.span);

    match &expr.kind {
        // ─────────────────────────────────────────────────────────────────────
        // Literals
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Unit => {
            fc.builder.emit_const(Value::Unit);
        }

        ExprKind::Bool(b) => {
            fc.builder.emit_const(Value::Bool(*b));
        }

        ExprKind::Number(n) => {
            fc.builder.emit_const(Value::Number(*n));
        }

        ExprKind::String(s) => {
            fc.builder.emit_const(str_to_value(s));
        }

        // ─────────────────────────────────────────────────────────────────────
        // Variables
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Local(id) => {
            // Check if this is a captured variable from the parent scope.
            if let Some(&capture_slot) = fc.captures.get(id) {
                fc.builder.emit_load_capture(capture_slot);
            } else if fc.is_parent_binding(*id) {
                // This is a free variable from the parent - add it as a capture.
                // Find the name for this binding from parent.
                let name = fc
                    .parent_locals
                    .as_ref()
                    .and_then(|_| {
                        // Look up name from parent_local_names by finding which name maps to the same slot
                        fc.parent_local_names.as_ref().and_then(|names| {
                            names.iter().find(|(_, &slot)| {
                                fc.parent_locals
                                    .as_ref()
                                    .is_some_and(|pl| pl.get(id).copied() == Some(slot))
                            })
                        })
                    })
                    .map_or_else(|| format!("__binding_{id}").into(), |(n, _)| Arc::clone(n));
                let capture_slot = fc.get_or_create_capture(*id, name);
                fc.builder.emit_load_capture(capture_slot);
            } else {
                let slot = fc.get_local(*id, (expr.span.start, expr.span.end))?;
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
            }
        }

        ExprKind::Name(name) => {
            // First check if it's a local variable (parameter or let binding).
            let var_name = &name.name;
            if let Some(slot) = fc.get_local_by_name(var_name) {
                // Load the local variable.
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
            } else if let Some(&capture_slot) = fc.capture_names.get(var_name) {
                // Load from captured environment.
                fc.builder.emit_load_capture(capture_slot);
            } else if fc.is_parent_name(var_name) {
                // This is a free variable from the parent - add it as a capture.
                let capture_slot = fc.get_or_create_capture_by_name(Arc::clone(var_name));
                fc.builder.emit_load_capture(capture_slot);
            } else if let Some(&hash) = fc.function_hashes.get(var_name) {
                // Check if it's a constant (thunk) that should be auto-called.
                if fc.is_repl_constant(var_name) {
                    // Call the constant thunk with no arguments to get its value.
                    fc.builder.emit_call(hash, 0);
                } else {
                    // It's a function reference - push it for later use.
                    fc.builder.emit_const(Value::FunctionRef(hash));
                }
            } else {
                return Err(CompileError::new(
                    CompileErrorKind::UndefinedFunction {
                        name: Arc::clone(var_name),
                    },
                    (expr.span.start, expr.span.end),
                ));
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Compound expressions
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Tuple(elements) => {
            for elem in elements {
                compile_expr(fc, elem, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeTuple, elements.len() as u8);
        }

        ExprKind::TupleIndex(tuple, index) => {
            compile_expr(fc, tuple, ctx)?;
            fc.builder.emit_u8(Opcode::TupleGet, *index as u8);
        }

        ExprKind::Record(fields) => {
            // Push field names and values interleaved.
            for (name, value) in fields {
                fc.builder.emit_const(str_to_value(name));
                compile_expr(fc, value, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeRecord, fields.len() as u8);
        }

        ExprKind::RecordField(record, field) => {
            compile_expr(fc, record, ctx)?;
            let idx = fc.builder.add_constant(str_to_value(field));
            fc.builder.emit_u16(Opcode::RecordGet, idx);
        }

        ExprKind::List(elements) => {
            for elem in elements {
                compile_expr(fc, elem, ctx)?;
            }
            fc.builder.emit_make_list(elements.len() as u16);
        }

        // ─────────────────────────────────────────────────────────────────────
        // Operators
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Binary(op, left, right) => {
            // Short-circuit evaluation for logical operators.
            match op {
                BinaryOp::And => {
                    compile_expr(fc, left, ctx)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right, ctx)?;
                    fc.builder.patch_jump(jump);
                }
                BinaryOp::Or => {
                    compile_expr(fc, left, ctx)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIf);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right, ctx)?;
                    fc.builder.patch_jump(jump);
                }
                _ => {
                    compile_expr(fc, left, ctx)?;
                    compile_expr(fc, right, ctx)?;
                    let opcode = match op {
                        BinaryOp::Add => Opcode::Add,
                        BinaryOp::Sub => Opcode::Sub,
                        BinaryOp::Mul => Opcode::Mul,
                        BinaryOp::Div => Opcode::Div,
                        BinaryOp::Mod => Opcode::Mod,
                        BinaryOp::Eq => Opcode::Eq,
                        BinaryOp::Ne => Opcode::Ne,
                        BinaryOp::Lt => Opcode::Lt,
                        BinaryOp::Le => Opcode::Le,
                        BinaryOp::Gt => Opcode::Gt,
                        BinaryOp::Ge => Opcode::Ge,
                        BinaryOp::And | BinaryOp::Or => unreachable!(),
                    };
                    fc.builder.emit(opcode);
                }
            }
        }

        ExprKind::Unary(op, operand) => {
            compile_expr(fc, operand, ctx)?;
            let opcode = match op {
                UnaryOp::Neg => Opcode::Neg,
                UnaryOp::Not => Opcode::Not,
            };
            fc.builder.emit(opcode);
        }

        // ─────────────────────────────────────────────────────────────────────
        // Control flow
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::If(cond, then_branch, else_branch) => {
            compile_expr(fc, cond, ctx)?;

            let else_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
            compile_expr(fc, then_branch, ctx)?;

            if let Some(else_expr) = else_branch {
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                fc.builder.patch_jump(else_jump);
                compile_expr(fc, else_expr, ctx)?;
                fc.builder.patch_jump(end_jump);
            } else {
                // No else branch - if condition is false, push unit.
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                fc.builder.patch_jump(else_jump);
                fc.builder.emit_const(Value::Unit);
                fc.builder.patch_jump(end_jump);
            }
        }

        ExprKind::Match(scrutinee, arms) => {
            compile_match(fc, scrutinee, arms, (expr.span.start, expr.span.end), ctx)?;
        }

        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                compile_stmt(fc, stmt, ctx)?;
            }
            if let Some(result_expr) = result {
                compile_expr(fc, result_expr, ctx)?;
            } else {
                fc.builder.emit_const(Value::Unit);
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Functions and calls
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Lambda(lambda) => {
            // Compile the lambda body as a separate function.
            // The closure will capture variables from the enclosing scope.
            compile_lambda(fc, lambda, ctx)?;
        }

        ExprKind::Call(callee, args) => {
            // Check if this is a direct call to a known function or an indirect call.
            if let ExprKind::Name(name) = &callee.kind {
                // Check for intrinsic functions first
                if try_compile_intrinsic(fc, name, args, ctx)?.is_some() {
                    // Intrinsic was compiled, nothing more to do
                } else if fc.function_hashes.contains_key(&name.name) {
                    // Direct function call to a known function.
                    // Compile arguments first (left to right).
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    let hash = fc.function_hashes[&name.name];
                    fc.builder.emit_call(hash, args.len() as u8);
                } else if fc.get_local_by_name(&name.name).is_some()
                    || fc.capture_names.contains_key(&name.name)
                    || fc.is_parent_name(&name.name)
                {
                    // Indirect call through a closure stored in a variable.
                    // First compile the closure (callee), then arguments.
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                } else {
                    // Unknown function - will error at runtime
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                }
            } else {
                // General indirect call (e.g., calling a lambda inline or result of expression).
                // Compile callee first, then arguments.
                compile_expr(fc, callee, ctx)?;
                for arg in args {
                    compile_expr(fc, arg, ctx)?;
                }
                fc.builder.emit_call_closure(args.len() as u8);
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Abilities
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Perform(ability_call) => {
            compile_ability_call(fc, ability_call, true, ctx)?;
        }

        ExprKind::Suspend(ability_call) => {
            compile_ability_call(fc, ability_call, false, ctx)?;
        }

        ExprKind::Handle(handle_expr) => {
            compile_handle_expr(fc, handle_expr, ctx)?;
        }

        ExprKind::Resume(value) => {
            // Resume transfers control back to a captured continuation.
            // In a handler function, the continuation is passed as the first argument (local slot 0).
            //
            // Compile to:
            // 1. Load continuation from local slot 0
            // 2. Push the resume value
            // 3. Emit Resume opcode

            // Load continuation from local slot 0 (implicit first parameter in handlers)
            fc.builder.emit_u16(Opcode::LoadLocal, 0);

            // Compile the value to resume with
            compile_expr(fc, value, ctx)?;

            // Emit Resume opcode
            fc.builder.emit(Opcode::Resume);
        }

        ExprKind::HandlerLiteral(handler_lit) => {
            compile_handler_literal(fc, expr, handler_lit, ctx)?;
        }

        ExprKind::Sandbox(sandbox_expr) => {
            // Sandbox compilation (Milestone 14)
            //
            // Sandboxing is a compile-time feature - the type checker has already
            // verified that the body only uses allowed abilities. At runtime,
            // we simply execute the body with no special handling.
            //
            // The type system ensures:
            // - Only allowed abilities are used within the sandbox body
            // - Unknown abilities in the `with` clause are rejected
            //
            // Future enhancements could add runtime enforcement via:
            // - Handler frame markers to prevent ability escalation
            // - Dynamic capability tracking
            compile_expr(fc, &sandbox_expr.body, ctx)?;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Statement Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile a statement.
fn compile_stmt(
    fc: &mut FunctionCompiler,
    stmt: &Stmt,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    match &stmt.kind {
        StmtKind::Let(let_binding) => {
            compile_let(fc, let_binding, ctx)?;
        }
        StmtKind::Expr(expr) => {
            compile_expr(fc, expr, ctx)?;
            // Discard the result of expression statements.
            fc.builder.emit(Opcode::Pop);
        }
    }
    Ok(())
}

/// Compile a let binding.
fn compile_let(
    fc: &mut FunctionCompiler,
    binding: &LetBinding,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile the initializer.
    compile_expr(fc, &binding.init, ctx)?;

    // Allocate a local slot and store the value.
    let slot = fc.alloc_local_with_name(binding.id, &binding.name)?;
    fc.builder.emit_u16(Opcode::StoreLocal, slot);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{FunctionDef, Item, Param, Span};

    fn test_span() -> Span {
        Span::default()
    }

    /// Helper to compile a single function for testing.
    fn compile_test_function(func: &FunctionDef) -> Result<CompiledFunction, CompileError> {
        let mut hashes = HashMap::new();
        let hash = compute_temporary_hash(&func.name);
        hashes.insert(Arc::clone(&func.name), hash);
        let mut ctx = ModuleContext::new();
        compile_function_with_hash(func, &hashes, &mut ctx, None, None)
    }

    #[test]
    fn test_compile_simple_function() {
        // fn add(x, y) { x + y }
        let func = FunctionDef {
            name: Arc::from("add"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x"), Param::new(1, "y")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");

        assert_eq!(compiled.param_count, 2);
        assert!(compiled.local_count >= 2);
    }

    #[test]
    fn test_compile_literals() {
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::number(42.0),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");

        // Should have the number constant in the pool.
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Number(n) if (n - 42.0).abs() < f64::EPSILON)));
    }

    #[test]
    fn test_compile_if_else() {
        // fn test(x) { if x { 1 } else { 2 } }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::if_then_else(Expr::local(0), Expr::number(1.0), Some(Expr::number(2.0))),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");

        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_module_with_functions() {
        let module = Module {
            name: Arc::from("test"),
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("double"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "x")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(BinaryOp::Mul, Expr::local(0), Expr::number(2.0)),
                    }),
                    test_span(),
                ),
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("run"),
                        name_span: Span::default(),
                        is_public: true,
                        type_params: vec![],
                        params: vec![],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::call(Expr::name("double"), vec![Expr::number(21.0)]),
                    }),
                    test_span(),
                ),
            ],
        };

        let compiled = compile_module(&module).expect("compilation failed");

        assert!(compiled.entry_point.is_some());
        assert!(compiled.get_function("double").is_some());
        assert!(compiled.get_function("run").is_some());
    }

    #[test]
    fn test_content_addressed_hash_identical_functions() {
        // Two modules with identical functions but different names should produce
        // the same content hash for those functions.
        let module1 = Module {
            name: Arc::from("test1"),
            items: vec![Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("add_one"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
                }),
                test_span(),
            )],
        };

        let module2 = Module {
            name: Arc::from("test2"),
            items: vec![Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("increment"), // Different name, same implementation
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
                }),
                test_span(),
            )],
        };

        let compiled1 = compile_module(&module1).expect("module1 compilation failed");
        let compiled2 = compile_module(&module2).expect("module2 compilation failed");

        let func1 = compiled1
            .get_function("add_one")
            .expect("add_one not found");
        let func2 = compiled2
            .get_function("increment")
            .expect("increment not found");

        // Content-addressed: identical bytecode should produce identical hash
        assert_eq!(
            func1.hash, func2.hash,
            "Identical functions with different names should have the same content hash"
        );
    }

    #[test]
    fn test_content_addressed_hash_different_functions() {
        // Functions with different implementations should have different hashes.
        let module = Module {
            name: Arc::from("test"),
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("add_one"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "x")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
                    }),
                    test_span(),
                ),
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("add_two"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "x")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(2.0)),
                    }),
                    test_span(),
                ),
            ],
        };

        let compiled = compile_module(&module).expect("compilation failed");

        let func1 = compiled.get_function("add_one").expect("add_one not found");
        let func2 = compiled.get_function("add_two").expect("add_two not found");

        // Different implementations should have different hashes
        assert_ne!(
            func1.hash, func2.hash,
            "Functions with different implementations should have different hashes"
        );
    }

    #[test]
    fn test_recursive_function_hash() {
        // A self-recursive function should get a stable hash
        let module = Module {
            name: Arc::from("test"),
            items: vec![Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("factorial"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "n")],
                    ret_ty: None,
                    abilities: vec![],
                    // if n <= 1 { 1 } else { n * factorial(n - 1) }
                    body: Expr::if_then_else(
                        Expr::binary(BinaryOp::Le, Expr::local(0), Expr::number(1.0)),
                        Expr::number(1.0),
                        Some(Expr::binary(
                            BinaryOp::Mul,
                            Expr::local(0),
                            Expr::call(
                                Expr::name("factorial"),
                                vec![Expr::binary(
                                    BinaryOp::Sub,
                                    Expr::local(0),
                                    Expr::number(1.0),
                                )],
                            ),
                        )),
                    ),
                }),
                test_span(),
            )],
        };

        let compiled = compile_module(&module).expect("compilation failed");
        let func = compiled
            .get_function("factorial")
            .expect("factorial not found");

        // Verify the hash is deterministic - compile again and check
        let compiled2 = compile_module(&module).expect("compilation failed");
        let func2 = compiled2
            .get_function("factorial")
            .expect("factorial not found");

        assert_eq!(
            func.hash, func2.hash,
            "Recursive function hash should be deterministic"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Enum Pattern Matching Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_compile_match_none_pattern() {
        use crate::ast::{MatchArm, Pattern};

        // fn test(x) {
        //   match x {
        //     None => 0,
        //     _ => 1,
        //   }
        // }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::match_expr(
                Expr::local(0),
                vec![
                    MatchArm::new(Pattern::variant("None", None), Expr::number(0.0)),
                    MatchArm::new(Pattern::wildcard(), Expr::number(1.0)),
                ],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_match_some_pattern() {
        use crate::ast::{MatchArm, Pattern};

        // fn test(x) {
        //   match x {
        //     Some(v) => v,
        //     None => 0,
        //   }
        // }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::match_expr(
                Expr::local(0),
                vec![
                    MatchArm::new(
                        Pattern::variant("Some", Some(Pattern::binding(1, "v"))),
                        Expr::local(1),
                    ),
                    MatchArm::new(Pattern::variant("None", None), Expr::number(0.0)),
                ],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
        // Should have at least 2 locals (param x and binding v)
        assert!(compiled.local_count >= 2);
    }

    #[test]
    fn test_compile_match_result_patterns() {
        use crate::ast::{MatchArm, Pattern};

        // fn test(x) {
        //   match x {
        //     Ok(v) => v,
        //     Err(e) => e,
        //   }
        // }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::match_expr(
                Expr::local(0),
                vec![
                    MatchArm::new(
                        Pattern::variant("Ok", Some(Pattern::binding(1, "v"))),
                        Expr::local(1),
                    ),
                    MatchArm::new(
                        Pattern::variant("Err", Some(Pattern::binding(2, "e"))),
                        Expr::local(2),
                    ),
                ],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_match_variant_with_wildcard_inner() {
        use crate::ast::{MatchArm, Pattern};

        // fn test(x) {
        //   match x {
        //     Some(_) => true,
        //     None => false,
        //   }
        // }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::match_expr(
                Expr::local(0),
                vec![
                    MatchArm::new(
                        Pattern::variant("Some", Some(Pattern::wildcard())),
                        Expr::bool(true),
                    ),
                    MatchArm::new(Pattern::variant("None", None), Expr::bool(false)),
                ],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_debug_info_generation() {
        // Test that source maps are generated when source is provided
        let source = r#"fn add(x, y) { x + y }"#;
        let source_file = "test.ab";

        // Create a function with spans that match the source
        let func = FunctionDef {
            name: Arc::from("add"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x"), Param::new(1, "y")],
            ret_ty: None,
            abilities: vec![],
            // The body expression has a span covering "x + y"
            body: Expr::new(
                ExprKind::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::local(0)),
                    Box::new(Expr::local(1)),
                ),
                Span::new(15, 20), // "x + y" in the source
            ),
        };

        let mut hashes = HashMap::new();
        let hash = compute_temporary_hash(&func.name);
        hashes.insert(Arc::clone(&func.name), hash);
        let mut ctx = ModuleContext::new();

        let compiled =
            compile_function_with_hash(&func, &hashes, &mut ctx, Some(source), Some(source_file))
                .expect("compilation failed");

        // Debug info should be present
        let debug_info = compiled.debug_info.expect("debug info should be generated");

        // Check function name
        assert_eq!(debug_info.function_name.as_deref(), Some("add"));

        // Check source file
        assert_eq!(debug_info.source_file.as_deref(), Some(source_file));

        // Check that source mappings were generated
        assert!(
            !debug_info.source_map.is_empty(),
            "source map should not be empty"
        );

        // Check that line/column were computed (line 1 since it's a single line)
        let first_mapping = &debug_info.source_map[0];
        assert_eq!(first_mapping.line, 1, "should be on line 1");
        assert!(
            first_mapping.column > 0,
            "column should be positive (1-indexed)"
        );

        // Check that local variable names were recorded
        assert!(
            debug_info.local_names.contains_key(&0),
            "local 'x' should be recorded"
        );
        assert!(
            debug_info.local_names.contains_key(&1),
            "local 'y' should be recorded"
        );
    }

    #[test]
    fn test_span_to_line_col() {
        let source = "line one\nline two\nline three";

        // Line 1, column 1
        let (line, col) = span_to_line_col(source, Span::new(0, 1));
        assert_eq!((line, col), (1, 1));

        // Line 1, column 5 ("one")
        let (line, col) = span_to_line_col(source, Span::new(5, 8));
        assert_eq!((line, col), (1, 6));

        // Line 2, column 1
        let (line, col) = span_to_line_col(source, Span::new(9, 10));
        assert_eq!((line, col), (2, 1));

        // Line 3, column 6 ("three")
        let (line, col) = span_to_line_col(source, Span::new(23, 28));
        assert_eq!((line, col), (3, 6));
    }
}
