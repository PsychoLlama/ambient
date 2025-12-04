//! Compiler that transforms typed AST into bytecode.
//!
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
//! # Bytecode Emission Patterns
//!
//! ## Expressions
//!
//! Each expression type compiles to a sequence of bytecode that leaves
//! one value on the stack:
//!
//! - **Literals**: `PushConst index` - push constant from pool
//! - **Binary ops**: compile left, compile right, emit op (e.g., `Add`)
//! - **Variables**: `LoadLocal slot` or `LoadCapture slot` for closures
//! - **Function calls**: push args, `Call arity`, result on stack
//!
//! ## Control Flow
//!
//! - **If/else**: compile cond, `JumpIfFalse else_label`, compile then,
//!   `Jump end_label`, `else_label:`, compile else, `end_label:`
//! - **Block**: compile each statement, final expression result on stack
//!
//! ## Closures
//!
//! Closures capture variables from enclosing scopes:
//! 1. Push captured values onto stack in slot order
//! 2. `MakeClosure func_hash capture_count`
//! 3. At call site: `LoadCapture slot` to access captured values
//!
//! ## Content-Addressed Functions
//!
//! Functions are identified by their `blake3::Hash`. The compiler:
//! 1. Assigns temporary hashes during compilation
//! 2. Computes final content-addressed hashes after all functions compiled
//! 3. Handles mutual recursion via strongly-connected component analysis
//!
//! # Internal Organization
//!
//! This file is organized into the following sections:
//!
//! - **Handler implicit parameter names**: Constants for implicit handler parameters
//! - **Compilation Errors**: `CompileError` and `CompileErrorKind` types
//! - **Compiled Module**: `CompiledModule` struct for compilation output
//! - **Compiler State**: `Compiler` struct with scoping and local variable management
//! - **Module Compilation**: Top-level compilation of modules, functions, and definitions
//! - **Expression Compilation**: Bytecode generation for all expression types
//! - **Statement Compilation**: Bytecode generation for statements
//! - **Pattern Matching Compilation**: Pattern matching code generation
//! - **REPL Support**: Interactive compilation utilities
//! - **Tests**: Unit tests for the compiler

#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{
    BinaryOp, BindingId, ConstDef, Expr, ExprKind, FunctionDef, ItemKind, LetBinding, Literal,
    MatchArm, Module, Pattern, PatternKind, Stmt, StmtKind, UnaryOp,
};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, Opcode};
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

// ─────────────────────────────────────────────────────────────────────────────
// Compilation Errors
// ─────────────────────────────────────────────────────────────────────────────

/// An error that occurred during compilation.
#[derive(Debug, Clone)]
pub struct CompileError {
    /// The kind of error.
    pub kind: CompileErrorKind,
    /// Source location (byte offset range).
    pub span: (u32, u32),
}

impl CompileError {
    /// Create a new compile error.
    #[must_use]
    pub fn new(kind: CompileErrorKind, span: (u32, u32)) -> Self {
        Self { kind, span }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for CompileError {}

/// The kind of compilation error.
#[derive(Debug, Clone)]
pub enum CompileErrorKind {
    /// Undefined function reference.
    UndefinedFunction { name: Arc<str> },

    /// Undefined local variable.
    UndefinedLocal { id: BindingId },

    /// Too many local variables.
    TooManyLocals { count: usize },

    /// Too many constants.
    TooManyConstants { count: usize },

    /// Unsupported expression (not yet implemented).
    Unsupported { feature: String },

    /// Ability not registered.
    UnknownAbility { name: Arc<str> },

    /// Unknown ability method.
    UnknownAbilityMethod { ability: Arc<str>, method: Arc<str> },

    /// Internal compiler error (invariant violation).
    Internal { message: &'static str },
}

impl std::fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UndefinedFunction { name } => write!(f, "undefined function: `{name}`"),
            Self::UndefinedLocal { id } => write!(f, "undefined local variable: binding {id}"),
            Self::TooManyLocals { count } => {
                write!(f, "too many local variables: {count} (max 65535)")
            }
            Self::TooManyConstants { count } => {
                write!(f, "too many constants: {count} (max 65535)")
            }
            Self::Unsupported { feature } => write!(f, "unsupported feature: {feature}"),
            Self::UnknownAbility { name } => write!(f, "unknown ability: `{name}`"),
            Self::UnknownAbilityMethod { ability, method } => {
                write!(f, "unknown ability method: `{ability}.{method}`")
            }
            Self::Internal { message } => write!(f, "internal compiler error: {message}"),
        }
    }
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

    /// The entry point function (typically "main").
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
        }
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
        }
    }

    /// Allocate a local slot for a binding with a name.
    fn alloc_local_with_name(
        &mut self,
        id: BindingId,
        name: Arc<str>,
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
        self.local_names.insert(name, slot);
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
    let mut temp_hashes: HashMap<Arc<str>, blake3::Hash> = HashMap::new();
    for func in &functions {
        let hash = compute_temporary_hash(&func.name);
        temp_hashes.insert(Arc::clone(&func.name), hash);
    }

    // Create module context for tracking lambdas discovered during compilation.
    let mut ctx = ModuleContext::new();

    // Phase 2: Compile each function using temporary hashes.
    let mut compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)> = Vec::new();
    for func in &functions {
        let compiled = compile_function_with_hash(func, &temp_hashes, &mut ctx)?;
        let is_main = &*func.name == "main";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Compile constants.
    for item in &module.items {
        if let ItemKind::Const(const_def) = &item.kind {
            let compiled = compile_const(const_def, &temp_hashes, &mut ctx)?;
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

    for scc in &scc_analysis.components {
        if scc.is_singleton() {
            // Single function - might be self-recursive or not
            let name = &scc.members[0];
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
            // Compute a group hash for the entire SCC
            let scc_hash = compute_scc_hash(
                &scc.members,
                &compiled_functions,
                &final_hashes,
                temp_hashes,
            );

            // Each function in the SCC gets a derived hash
            for (idx, name) in scc.members.iter().enumerate() {
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
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slots for parameters (with names for lookup).
    for param in &func.params {
        fc.alloc_local_with_name(param.id, Arc::clone(&param.name))?;
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

    Ok(CompiledFunction {
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
    })
}

/// Compile a constant definition to a function that returns the constant value.
fn compile_const(
    const_def: &ConstDef,
    function_hashes: &HashMap<Arc<str>, blake3::Hash>,
    ctx: &mut ModuleContext,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Compile the value expression.
    compile_expr(&mut fc, &const_def.value, ctx)?;

    // Return the value.
    fc.builder.emit(Opcode::Return);

    Ok(fc.builder.build(fc.next_local, 0))
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
                // It's a function reference.
                fc.builder.emit_const(Value::FunctionRef(hash));
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
            // Lists are represented as tuples for now.
            // TODO: Implement proper List type.
            for elem in elements {
                compile_expr(fc, elem, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeTuple, elements.len() as u8);
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
            match &callee.kind {
                ExprKind::Name(name) if fc.function_hashes.contains_key(&name.name) => {
                    // Direct function call to a known function.
                    // Compile arguments first (left to right).
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    let hash = fc.function_hashes[&name.name];
                    fc.builder.emit_call(hash, args.len() as u8);
                }
                ExprKind::Name(name)
                    if fc.get_local_by_name(&name.name).is_some()
                        || fc.capture_names.contains_key(&name.name)
                        || fc.is_parent_name(&name.name) =>
                {
                    // Indirect call through a closure stored in a variable.
                    // First compile the closure (callee), then arguments.
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                }
                _ => {
                    // General indirect call (e.g., calling a lambda inline or result of expression).
                    // Compile callee first, then arguments.
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                }
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
            // Compile handler literal (Milestone 13)
            //
            // Handler literals create a HandlerValue at runtime. Each method
            // is compiled as a separate function that receives implicit parameters:
            // - slot 0: continuation
            // - slot 1: suspended ability value
            // - slot 2+: extracted ability method arguments

            // Get the ability ID from the type (set by type checker).
            // The type should be Handler<ability_id>.
            let ability_id = match &expr.ty {
                Some(crate::types::Type::Handler(h)) => h.ability,
                _ => {
                    return Err(CompileError::new(
                        CompileErrorKind::Unsupported {
                            feature: "handler literal without type annotation".to_string(),
                        },
                        (expr.span.start, expr.span.end),
                    ));
                }
            };

            let ability_name = get_ability_name(ability_id).ok_or_else(|| {
                CompileError::new(
                    CompileErrorKind::Unsupported {
                        feature: format!("unknown ability ID: {ability_id}"),
                    },
                    (expr.span.start, expr.span.end),
                )
            })?;

            // Compile each handler method as a separate function.
            let mut method_hashes: Vec<(u16, blake3::Hash)> = Vec::new();

            for method in &handler_lit.methods {
                let method_id =
                    get_method_id_for_ability(ability_id, &method.method).ok_or_else(|| {
                        CompileError::new(
                            CompileErrorKind::Unsupported {
                                feature: format!(
                                    "unknown method `{}` for ability `{}`",
                                    method.method, ability_name
                                ),
                            },
                            (method.span.start, method.span.end),
                        )
                    })?;

                // Create a new FunctionCompiler for the handler method.
                let mut method_fc = FunctionCompiler::new_for_closure(
                    fc.function_hashes.clone(),
                    fc.locals.clone(),
                    fc.local_names.clone(),
                );

                // Allocate implicit slots for continuation and suspended ability.
                let _continuation_slot =
                    method_fc.alloc_local_with_name(0, Arc::from(HANDLER_PARAM_CONTINUATION))?;
                let _ability_slot = method_fc
                    .alloc_local_with_name(0, Arc::from(HANDLER_PARAM_SUSPENDED_ABILITY))?;

                // Extract ability arguments and store to param slots.
                for (i, param) in method.params.iter().enumerate() {
                    method_fc.alloc_local_with_name(param.id, Arc::clone(&param.name))?;

                    // Load suspended ability from slot 1.
                    method_fc.builder.emit_u16(Opcode::LoadLocal, 1);

                    // Extract argument at index i.
                    method_fc.builder.emit_get_ability_arg(i as u8);

                    // Store to the param slot (slot 2+i).
                    method_fc.builder.emit_u16(Opcode::StoreLocal, 2 + i as u16);
                }

                // Compile the handler method body.
                compile_expr(&mut method_fc, &method.body, ctx)?;

                // Emit return instruction.
                method_fc.builder.emit(Opcode::Return);

                // Build the handler method function.
                let bytecode = method_fc.builder.bytecode().to_vec();
                let constants = method_fc.builder.constants().to_vec();
                let dependencies = method_fc.builder.dependencies().to_vec();
                let local_count = method_fc.next_local;
                // Handler receives 2 implicit params: continuation and suspended ability.
                let param_count = 2;

                let method_func = CompiledFunction {
                    hash: ctx.next_lambda_hash(),
                    bytecode,
                    constants,
                    local_count,
                    param_count,
                    dependencies,
                };

                // Get capture info for the handler method.
                let capture_names = method_fc.get_capture_names_in_order();

                // Register the handler method function.
                let final_hash = ctx.register_lambda(method_func);

                // If handler method captures variables, emit code to load them.
                // This is similar to closure compilation.
                for (name, _slot) in &capture_names {
                    if let Some(&slot) = fc.local_names.get(name) {
                        fc.builder.emit_u16(Opcode::LoadLocal, slot);
                    } else if let Some(&capture_slot) = fc.capture_names.get(name) {
                        fc.builder.emit_load_capture(capture_slot);
                    }
                    // If not found, the capture tracking is wrong, but we continue.
                }

                method_hashes.push((method_id, final_hash));
            }

            // Calculate total capture count (if any methods capture variables).
            // For simplicity, we don't support handler method captures yet.
            let capture_count = 0u8;

            // Emit MakeHandler instruction.
            fc.builder
                .emit_make_handler(ability_id, &method_hashes, capture_count);
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

/// Compile a lambda expression.
///
/// This compiles the lambda body as a separate function and emits code
/// to create a closure value with captured variables.
fn compile_lambda(
    fc: &mut FunctionCompiler,
    lambda: &crate::ast::Lambda,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Create a new FunctionCompiler for the lambda body.
    // Pass the current scope's locals as the parent scope.
    let mut lambda_fc = FunctionCompiler::new_for_closure(
        fc.function_hashes.clone(),
        fc.locals.clone(),
        fc.local_names.clone(),
    );

    // Allocate slots for lambda parameters.
    for param in &lambda.params {
        lambda_fc.alloc_local_with_name(param.id, Arc::clone(&param.name))?;
    }

    // Compile the lambda body.
    // During this compilation, lambda_fc will track any captured variables.
    compile_expr(&mut lambda_fc, &lambda.body, ctx)?;

    // Emit return instruction.
    lambda_fc.builder.emit(Opcode::Return);

    // Build the compiled lambda function.
    let bytecode = lambda_fc.builder.bytecode().to_vec();
    let constants = lambda_fc.builder.constants().to_vec();
    let dependencies = lambda_fc.builder.dependencies().to_vec();
    let local_count = lambda_fc.next_local;
    let param_count = lambda.params.len() as u8;

    // Create the CompiledFunction with a temporary hash.
    let temp_hash = ctx.next_lambda_hash();
    let compiled_func = CompiledFunction {
        hash: temp_hash,
        bytecode,
        constants,
        local_count,
        param_count,
        dependencies,
    };

    // Get the captures in order (by name, since that's how ExprKind::Name captures work).
    let capture_names = lambda_fc.get_capture_names_in_order();
    let capture_count = capture_names.len();

    // Register the lambda with the module context.
    let lambda_hash = ctx.register_lambda(compiled_func);

    // Now emit code in the enclosing function to create the closure.
    // Push captured values onto the stack in capture slot order.
    for (name, _slot) in &capture_names {
        // Load the captured value from the current function's scope.
        if let Some(&slot) = fc.local_names.get(name) {
            fc.builder.emit_u16(Opcode::LoadLocal, slot);
        } else if let Some(&capture_slot) = fc.capture_names.get(name) {
            // If the enclosing function is itself a closure, load from its captures.
            fc.builder.emit_load_capture(capture_slot);
        } else {
            // Should not happen if capture analysis is correct.
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown capture: {name}"),
                },
                (0, 0),
            ));
        }
    }

    // 2. Emit MakeClosure instruction.
    fc.builder
        .emit_make_closure(lambda_hash, capture_count as u8);

    Ok(())
}

/// Compile a handle expression.
///
/// A handle expression installs ability handlers and executes a body expression.
/// Each handler is compiled as a separate function that receives:
/// - Local slot 0: the continuation (to resume with)
/// - Local slot 1: the suspended ability (containing method args)
/// - Local slots 2+: extracted ability arguments bound to handler params
fn compile_handle_expr(
    fc: &mut FunctionCompiler,
    handle_expr: &crate::ast::HandleExpr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    let mut handler_hashes = Vec::new();
    let mut handler_ability_ids = Vec::new();

    // Track jump offsets for handler values (from `with` clause)
    let mut handler_value_jump_offsets = Vec::new();

    // Compile handler values from `with` clause
    // Each handler value expression evaluates to a HandlerValue on the stack,
    // then HandleWithValue pops it and installs it as a handler.
    for handler_value in &handle_expr.handler_values {
        // Verify this is a Handler type (type checking should have caught errors)
        match &handler_value.ty {
            Some(crate::types::Type::Handler(_)) => {}
            _ => {
                // Skip if not a handler type - type checking should have caught this
                continue;
            }
        }

        // Compile the handler value expression - this leaves a HandlerValue on the stack.
        compile_expr(fc, handler_value, ctx)?;

        // Emit HandleWithValue to pop the handler and install it
        let jump_offset = fc.builder.emit_handle_with_value();
        handler_value_jump_offsets.push(jump_offset);
    }

    // First, compile each inline handler as a separate function.
    for handler in &handle_expr.handlers {
        // Get ability ID for this handler.
        let ability_name = &handler.ability.name;
        let ability_id = get_ability_id(ability_name).ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability: {ability_name}"),
                },
                (handler.span.start, handler.span.end),
            )
        })?;

        // Create a new FunctionCompiler for the handler.
        // Handler functions have implicit parameters:
        // - slot 0: continuation
        // - slot 1: suspended ability value
        // - slot 2+: ability method arguments
        let mut handler_fc = FunctionCompiler::new(fc.function_hashes.clone());

        // Allocate implicit slots for continuation and suspended ability.
        let _continuation_slot =
            handler_fc.alloc_local_with_name(0, Arc::from(HANDLER_PARAM_CONTINUATION))?;
        let _ability_slot =
            handler_fc.alloc_local_with_name(0, Arc::from(HANDLER_PARAM_SUSPENDED_ABILITY))?;

        // At the start of the handler, extract ability arguments and store to param slots.
        // For each param, we need to:
        // 1. Load the suspended ability from slot 1
        // 2. Extract the argument at the corresponding index
        // 3. Store to the param's slot
        for (i, param) in handler.params.iter().enumerate() {
            // Allocate slot for this param.
            handler_fc.alloc_local_with_name(param.id, Arc::clone(&param.name))?;

            // Load suspended ability from slot 1.
            handler_fc.builder.emit_u16(Opcode::LoadLocal, 1);

            // Extract argument at index i.
            handler_fc.builder.emit_get_ability_arg(i as u8);

            // Store to the param slot (slot 2+i).
            handler_fc
                .builder
                .emit_u16(Opcode::StoreLocal, 2 + i as u16);
        }

        // Compile the handler body.
        compile_expr(&mut handler_fc, &handler.body, ctx)?;

        // Emit return instruction.
        handler_fc.builder.emit(Opcode::Return);

        // Build the handler function.
        let local_count = handler_fc.next_local;
        // Handler receives 2 implicit params: continuation and suspended ability.
        let param_count = 2;
        let handler_func = handler_fc.builder.build(local_count, param_count);
        let handler_hash = handler_func.hash;

        // Register the handler function.
        ctx.lambdas.push((handler_hash, handler_func));

        handler_hashes.push(handler_hash);
        handler_ability_ids.push(ability_id);
    }

    // Now emit the handle expression code.
    // Install handlers and compile the body.

    // Store the handle instruction jump offsets for patching.
    let mut handle_jump_offsets = Vec::new();

    // Emit Handle instructions for each handler.
    for (i, (ability_id, handler_hash)) in handler_ability_ids
        .iter()
        .zip(handler_hashes.iter())
        .enumerate()
    {
        let _ = i; // Silence unused warning.
        let jump_offset = fc.builder.emit_handle(*ability_id, *handler_hash);
        handle_jump_offsets.push(jump_offset);
    }

    // Compile the body expression.
    compile_expr(fc, &handle_expr.body, ctx)?;

    // Emit Unhandle for each handler (in reverse order).
    // First unhandle inline handlers (most recently installed)
    for _ in &handle_expr.handlers {
        fc.builder.emit(Opcode::Unhandle);
    }
    // Then unhandle handler values (installed first)
    for _ in &handle_expr.handler_values {
        fc.builder.emit(Opcode::Unhandle);
    }

    // Patch all the handle instruction jump offsets to point here.
    // Patch inline handler offsets
    for offset in handle_jump_offsets {
        fc.builder.patch_handle(offset);
    }
    // Patch handler value offsets
    for offset in handler_value_jump_offsets {
        fc.builder.patch_handle_with_value(offset);
    }

    // Handle else clause if present.
    if let Some(else_clause) = &handle_expr.else_clause {
        // The else clause wraps the result of normal completion.
        // For now, just compile it and drop the body result.
        fc.builder.emit(Opcode::Pop);
        compile_expr(fc, else_clause, ctx)?;
    }

    Ok(())
}

/// Compile an ability call (either perform or suspend).
fn compile_ability_call(
    fc: &mut FunctionCompiler,
    ability_call: &crate::ast::AbilityCall,
    perform: bool,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile arguments.
    for arg in &ability_call.args {
        compile_expr(fc, arg, ctx)?;
    }

    // Get ability and method IDs.
    // For now, use a simple mapping based on well-known abilities.
    let ability_name = &ability_call.ability.name;
    let method_name = &ability_call.method;

    let (ability_id, method_id) = get_ability_ids(ability_name, method_name).ok_or_else(|| {
        CompileError::new(
            CompileErrorKind::UnknownAbilityMethod {
                ability: Arc::clone(ability_name),
                method: Arc::clone(method_name),
            },
            (ability_call.span.start, ability_call.span.end),
        )
    })?;

    // Emit suspend instruction.
    fc.builder
        .emit_suspend(ability_id, method_id, ability_call.args.len() as u8);

    // If performing, emit perform instruction.
    if perform {
        fc.builder.emit(Opcode::Perform);
    }

    Ok(())
}

/// Get ability and method IDs for well-known abilities.
fn get_ability_ids(ability: &str, method: &str) -> Option<(u16, u16)> {
    use crate::abilities::{async_ability, console, exception, random, time};

    match (ability, method) {
        ("Console", "print") => Some((console::ABILITY_ID, console::METHOD_PRINT)),
        ("Console", "eprint") => Some((console::ABILITY_ID, console::METHOD_EPRINT)),
        ("Console", "println") => Some((console::ABILITY_ID, console::METHOD_PRINTLN)),
        ("Exception", "throw") => Some((exception::ABILITY_ID, exception::METHOD_THROW)),
        ("Time", "now") => Some((time::ABILITY_ID, time::METHOD_NOW)),
        ("Time", "wait") => Some((time::ABILITY_ID, time::METHOD_WAIT)),
        ("Random", "seed") => Some((random::ABILITY_ID, random::METHOD_SEED)),
        ("Random", "in_range") => Some((random::ABILITY_ID, random::METHOD_IN_RANGE)),
        ("Async", "all") => Some((async_ability::ABILITY_ID, async_ability::METHOD_ALL)),
        ("Async", "race") => Some((async_ability::ABILITY_ID, async_ability::METHOD_RACE)),
        _ => None,
    }
}

/// Get ability ID for a well-known ability.
fn get_ability_id(ability: &str) -> Option<u16> {
    use crate::abilities::{async_ability, console, exception, random, time};

    match ability {
        "Console" => Some(console::ABILITY_ID),
        "Exception" => Some(exception::ABILITY_ID),
        "Time" => Some(time::ABILITY_ID),
        "Random" => Some(random::ABILITY_ID),
        "Async" => Some(async_ability::ABILITY_ID),
        _ => None,
    }
}

/// Get method ID from ability ID and method name.
fn get_method_id_for_ability(ability_id: u16, method_name: &str) -> Option<u16> {
    use crate::abilities::{async_ability, console, exception, random, time};

    match ability_id {
        id if id == console::ABILITY_ID => match method_name {
            "print" => Some(console::METHOD_PRINT),
            "eprint" => Some(console::METHOD_EPRINT),
            "println" | "eprintln" => Some(console::METHOD_PRINTLN),
            _ => None,
        },
        id if id == exception::ABILITY_ID => match method_name {
            "throw" => Some(exception::METHOD_THROW),
            _ => None,
        },
        id if id == time::ABILITY_ID => match method_name {
            "now" => Some(time::METHOD_NOW),
            "wait" => Some(time::METHOD_WAIT),
            _ => None,
        },
        id if id == random::ABILITY_ID => match method_name {
            "seed" => Some(random::METHOD_SEED),
            "in_range" => Some(random::METHOD_IN_RANGE),
            _ => None,
        },
        id if id == async_ability::ABILITY_ID => match method_name {
            "all" => Some(async_ability::METHOD_ALL),
            "race" => Some(async_ability::METHOD_RACE),
            _ => None,
        },
        _ => None,
    }
}

/// Get ability name from ability ID.
fn get_ability_name(ability_id: u16) -> Option<&'static str> {
    use crate::abilities::{async_ability, console, exception, random, time};

    match ability_id {
        id if id == console::ABILITY_ID => Some("Console"),
        id if id == exception::ABILITY_ID => Some("Exception"),
        id if id == time::ABILITY_ID => Some("Time"),
        id if id == random::ABILITY_ID => Some("Random"),
        id if id == async_ability::ABILITY_ID => Some("Async"),
        _ => None,
    }
}

/// Get the variant tag for a well-known enum variant name.
///
/// Tags for Option:
/// - None = 0
/// - Some = 1
///
/// Tags for Result:
/// - Ok = 0
/// - Err = 1
fn get_variant_tag(variant_name: &str) -> Option<u16> {
    match variant_name {
        // Tag 0: unit/success variants
        "None" | "Ok" => Some(0),
        // Tag 1: payload/error variants
        "Some" | "Err" => Some(1),
        _ => None,
    }
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
    let slot = fc.alloc_local_with_name(binding.id, Arc::clone(&binding.name))?;
    fc.builder.emit_u16(Opcode::StoreLocal, slot);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Pattern Matching Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile a match expression.
fn compile_match(
    fc: &mut FunctionCompiler,
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: (u32, u32),
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    if arms.is_empty() {
        return Err(CompileError::new(
            CompileErrorKind::Unsupported {
                feature: "empty match expression".to_string(),
            },
            span,
        ));
    }

    // Compile the scrutinee.
    compile_expr(fc, scrutinee, ctx)?;

    // For now, only support simple patterns (wildcards, bindings, literals).
    // TODO: Full pattern matching with nested patterns and guards.

    let mut end_jumps = Vec::new();

    for (i, arm) in arms.iter().enumerate() {
        let is_last = i == arms.len() - 1;

        // Duplicate scrutinee for pattern matching (except last arm).
        if !is_last {
            fc.builder.emit(Opcode::Dup);
        }

        // Compile pattern match.
        let fail_jump = compile_pattern_match(fc, &arm.pattern, is_last)?;

        // If guard exists, compile it.
        if let Some(guard) = &arm.guard {
            compile_expr(fc, guard, ctx)?;
            if let Some(fj) = fail_jump {
                // If pattern matched but guard fails, need to jump to next arm.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                // Pattern and guard both succeeded - compile body.
                compile_expr(fc, &arm.body, ctx)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                fc.builder.patch_jump(fj);
            } else {
                // Last arm with guard.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                compile_expr(fc, &arm.body, ctx)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                // If guard fails on last arm, push unit as result.
                fc.builder.emit_const(Value::Unit);
            }
        } else if let Some(fj) = fail_jump {
            // Pattern match can fail, compile body and jump to end.
            compile_expr(fc, &arm.body, ctx)?;
            let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
            end_jumps.push(end_jump);
            fc.builder.patch_jump(fj);
        } else {
            // Pattern always matches (wildcard or binding) and it's the last arm.
            compile_expr(fc, &arm.body, ctx)?;
        }
    }

    // Patch all end jumps to here.
    for jump in end_jumps {
        fc.builder.patch_jump(jump);
    }

    Ok(())
}

/// Compile a pattern match. Returns jump offset if pattern can fail.
fn compile_pattern_match(
    fc: &mut FunctionCompiler,
    pattern: &Pattern,
    is_last: bool,
) -> Result<Option<usize>, CompileError> {
    match &pattern.kind {
        PatternKind::Wildcard => {
            // Wildcard always matches, consume the scrutinee.
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            Ok(None)
        }

        PatternKind::Binding(id, name) => {
            // Binding always matches, store in local.
            let slot = fc.alloc_local_with_name(*id, Arc::clone(name))?;
            fc.builder.emit_u16(Opcode::StoreLocal, slot);
            if !is_last {
                // Consume the duplicated scrutinee.
                fc.builder.emit(Opcode::Pop);
            }
            Ok(None)
        }

        PatternKind::Literal(lit) => {
            // Compare with literal.
            let value = match lit {
                Literal::Unit => Value::Unit,
                Literal::Bool(b) => Value::Bool(*b),
                Literal::Number(n) => Value::Number(*n),
                Literal::String(s) => str_to_value(s),
            };
            fc.builder.emit_const(value);
            fc.builder.emit(Opcode::Eq);
            let fail_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            Ok(Some(fail_jump))
        }

        PatternKind::Variant(variant_name, inner_pattern) => {
            // Enum variant pattern matching: `Some(x)`, `None`, `Ok(v)`, `Err(e)`
            //
            // Stack behavior (from VM dispatch.rs):
            // - EnumIs: uses peek(), pushes bool -> [enum] becomes [enum, bool]
            // - EnumPayload: pops enum, pushes payload -> [enum] becomes [payload]
            //
            // For non-last arms, compile_match does Dup first, so we have [orig, dup].
            // For last arm, we just have [orig].
            //
            // The challenge: EnumIs doesn't consume the enum, so on the fail path
            // we have [orig, dup, bool] -> JumpIfNot -> [orig, dup].
            // But the next arm expects [orig] to Dup from.
            //
            // Solution: On fail path, pop the dup before continuing to next arm.
            // We emit:
            //   EnumIs           -> [orig, dup, bool] or [orig, bool]
            //   JumpIfNot cleanup_and_fail
            //   <success code>
            //   Jump past_cleanup
            //   cleanup_and_fail: Pop  (remove dup on fail path)
            //   past_cleanup: <returned as fail_jump for compile_match to patch>

            let tag = get_variant_tag(&variant_name.name).ok_or_else(|| {
                CompileError::new(
                    CompileErrorKind::Unsupported {
                        feature: format!("unknown variant: {}", variant_name.name),
                    },
                    (pattern.span.start, pattern.span.end),
                )
            })?;

            // Check if the enum matches this variant tag.
            // EnumIs: [enum] -> [enum, bool]
            fc.builder.emit_enum_is(tag);

            // Jump to cleanup on fail
            let cleanup_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);

            // === SUCCESS PATH ===
            // Stack after JumpIfNot (not taken): [orig, dup] or [orig]
            // (JumpIfNot consumed the bool)

            if let Some(inner) = inner_pattern {
                // Extract payload: [orig, dup] -> [orig, payload]
                fc.builder.emit_enum_payload();

                // Recursively match the inner pattern against the payload.
                // is_last=true because we want inner match to consume the payload.
                compile_pattern_match(fc, inner, true)?;
                // Stack: [orig] (non-last) or [] (last)
            } else {
                // Unit variant (like None) - pop the matched enum
                fc.builder.emit(Opcode::Pop);
                // Stack: [orig] (non-last) or [] (last)
            }

            // For non-last arms, pop the original scrutinee (compile_match expects [])
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            // Stack: []

            // Jump past the cleanup code
            let past_cleanup_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);

            // === FAIL PATH (cleanup) ===
            fc.builder.patch_jump(cleanup_jump);
            // Stack here: [orig, dup] (non-last) or [orig] (last)
            // JumpIfNot consumed the bool

            // Pop the enum that EnumIs left on stack
            fc.builder.emit(Opcode::Pop);
            // Stack: [orig] (non-last) or [] (last)

            // For non-last: we now have [orig] which is correct for next arm's Dup
            // For last: we have [] but there's no next arm anyway

            // Patch the success path's jump to skip over the fail cleanup
            fc.builder.patch_jump(past_cleanup_jump);

            // Return the fail target for compile_match to use
            // But wait - we need to return a jump placeholder, not an offset.
            // The issue is compile_match expects a jump that IT will patch.
            //
            // Actually looking at compile_match:
            //   let fail_jump = compile_pattern_match(...)?;
            //   ...compile body...
            //   fc.builder.patch_jump(fj);  // patches to HERE
            //
            // So fail_jump should point to AFTER the body. But we've already
            // handled the fail path cleanup inline. We need a different approach.
            //
            // New approach: Don't return a fail_jump. Instead, emit a Jump
            // placeholder that we return, which compile_match will patch.
            // On fail, we clean up and then fall through to this Jump.

            // Actually, let me re-read how this works. Looking at the patching:
            //   fc.builder.patch_jump(fj);
            // This patches fj to the current bytecode position.
            //
            // So if I return a Jump placeholder here, compile_match will patch
            // it to point to after the arm body. That's what we want!

            // Emit a jump that compile_match will patch to the right place
            let fail_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);

            Ok(Some(fail_jump))
        }

        PatternKind::Tuple(_) | PatternKind::Record(_) => Err(CompileError::new(
            CompileErrorKind::Unsupported {
                feature: "complex patterns (tuple/record)".to_string(),
            },
            (pattern.span.start, pattern.span.end),
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL Support
// ─────────────────────────────────────────────────────────────────────────────

/// Compile a standalone expression for REPL evaluation.
///
/// This wraps the expression in an anonymous function and compiles it.
/// The function takes no parameters and returns the expression's value.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_expression(expr: &Expr) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(HashMap::new());
    let mut ctx = ModuleContext::new();

    // Compile the expression (leaves result on stack).
    compile_expr(&mut fc, expr, &mut ctx)?;

    // Emit return instruction.
    fc.builder.emit(Opcode::Return);

    // Build with no parameters.
    Ok(fc.builder.build(fc.next_local, 0))
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
        compile_function_with_hash(func, &hashes, &mut ctx)
    }

    #[test]
    fn test_compile_simple_function() {
        // fn add(x, y) { x + y }
        let func = FunctionDef {
            name: Arc::from("add"),
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
                        name: Arc::from("main"),
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
        assert!(compiled.get_function("main").is_some());
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
}
