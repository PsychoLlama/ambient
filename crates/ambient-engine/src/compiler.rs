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

#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{
    BinaryOp, BindingId, ConstDef, Expr, ExprKind, FunctionDef, ItemKind, LetBinding, Literal,
    MatchArm, Module, Pattern, PatternKind, Stmt, StmtKind, UnaryOp,
};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, Opcode};
use crate::value::Value;

/// Helper to convert Arc<str> to Value::String (which uses Arc<String>).
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
        }
    }

    /// Allocate a local slot for a binding with a name.
    fn alloc_local_with_name(
        &mut self,
        id: BindingId,
        name: Arc<str>,
    ) -> Result<u16, CompileError> {
        if self.next_local >= u16::MAX {
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

    // Phase 2: Compile each function using temporary hashes.
    let mut compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)> = Vec::new();
    for func in &functions {
        let compiled = compile_function_with_hash(func, &temp_hashes)?;
        let is_main = &*func.name == "main";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Compile constants.
    for item in &module.items {
        if let ItemKind::Const(const_def) = &item.kind {
            let compiled = compile_const(const_def, &temp_hashes)?;
            compiled_functions.push((Arc::clone(&const_def.name), compiled, false));
        }
    }

    // Phase 3: Compute content-addressed hashes and finalize the module.
    finalize_module_hashes(compiled_functions, temp_hashes)
}

/// Finalize content-addressed hashes for all compiled functions.
///
/// This handles:
/// 1. Non-recursive functions: compute hash from bytecode content
/// 2. Recursive functions (SCCs): compute group hash for mutual recursion
fn finalize_module_hashes(
    compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)>,
    temp_hashes: HashMap<Arc<str>, blake3::Hash>,
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
                .expect("function should exist");

            // Check if it's self-recursive
            let is_self_recursive = call_graph
                .get(name)
                .map(|calls| calls.contains(name))
                .unwrap_or(false);

            if is_self_recursive {
                // Self-recursive: compute hash excluding self-reference
                let hash = compute_scc_hash(&scc.members, &compiled_functions, &final_hashes, &temp_hashes);
                final_hashes.insert(Arc::clone(name), hash);
            } else {
                // Non-recursive: compute hash with resolved dependencies
                let hash = compute_content_hash(func, &final_hashes, &temp_hashes);
                final_hashes.insert(Arc::clone(name), hash);
            }
        } else {
            // Multiple functions in SCC - mutual recursion
            // Compute a group hash for the entire SCC
            let scc_hash = compute_scc_hash(&scc.members, &compiled_functions, &final_hashes, &temp_hashes);

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
        let final_hash = final_hashes
            .get(&name)
            .copied()
            .expect("all functions should have final hashes");

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
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slots for parameters (with names for lookup).
    for param in &func.params {
        fc.alloc_local_with_name(param.id, Arc::clone(&param.name))?;
    }

    // Compile the function body.
    compile_expr(&mut fc, &func.body)?;

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
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Compile the value expression.
    compile_expr(&mut fc, &const_def.value)?;

    // Return the value.
    fc.builder.emit(Opcode::Return);

    Ok(fc.builder.build(fc.next_local, 0))
}

// ─────────────────────────────────────────────────────────────────────────────
// Expression Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile an expression, pushing its value onto the stack.
fn compile_expr(fc: &mut FunctionCompiler, expr: &Expr) -> Result<(), CompileError> {
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
            let slot = fc.get_local(*id, (expr.span.start, expr.span.end))?;
            fc.builder.emit_u16(Opcode::LoadLocal, slot);
        }

        ExprKind::Name(name) => {
            // First check if it's a local variable (parameter or let binding).
            let var_name = &name.name;
            if let Some(slot) = fc.get_local_by_name(var_name) {
                // Load the local variable.
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
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
                compile_expr(fc, elem)?;
            }
            fc.builder.emit_u8(Opcode::MakeTuple, elements.len() as u8);
        }

        ExprKind::TupleIndex(tuple, index) => {
            compile_expr(fc, tuple)?;
            fc.builder.emit_u8(Opcode::TupleGet, *index as u8);
        }

        ExprKind::Record(fields) => {
            // Push field names and values interleaved.
            for (name, value) in fields {
                fc.builder.emit_const(str_to_value(name));
                compile_expr(fc, value)?;
            }
            fc.builder.emit_u8(Opcode::MakeRecord, fields.len() as u8);
        }

        ExprKind::RecordField(record, field) => {
            compile_expr(fc, record)?;
            let idx = fc.builder.add_constant(str_to_value(field));
            fc.builder.emit_u16(Opcode::RecordGet, idx);
        }

        ExprKind::List(elements) => {
            // Lists are represented as tuples for now.
            // TODO: Implement proper List type.
            for elem in elements {
                compile_expr(fc, elem)?;
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
                    compile_expr(fc, left)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right)?;
                    fc.builder.patch_jump(jump);
                }
                BinaryOp::Or => {
                    compile_expr(fc, left)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIf);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right)?;
                    fc.builder.patch_jump(jump);
                }
                _ => {
                    compile_expr(fc, left)?;
                    compile_expr(fc, right)?;
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
            compile_expr(fc, operand)?;
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
            compile_expr(fc, cond)?;

            let else_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
            compile_expr(fc, then_branch)?;

            if let Some(else_expr) = else_branch {
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                fc.builder.patch_jump(else_jump);
                compile_expr(fc, else_expr)?;
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
            compile_match(fc, scrutinee, arms, (expr.span.start, expr.span.end))?;
        }

        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                compile_stmt(fc, stmt)?;
            }
            if let Some(result_expr) = result {
                compile_expr(fc, result_expr)?;
            } else {
                fc.builder.emit_const(Value::Unit);
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Functions and calls
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Lambda(_lambda) => {
            // For now, lambdas are not yet supported.
            // TODO: Implement closure compilation.
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: "lambda expressions (closures)".to_string(),
                },
                (expr.span.start, expr.span.end),
            ));
        }

        ExprKind::Call(callee, args) => {
            // Compile arguments first (left to right).
            for arg in args {
                compile_expr(fc, arg)?;
            }

            // Compile the callee and call it.
            match &callee.kind {
                ExprKind::Name(name) => {
                    // Direct function call.
                    if let Some(&hash) = fc.function_hashes.get(&name.name) {
                        fc.builder.emit_call(hash, args.len() as u8);
                    } else {
                        return Err(CompileError::new(
                            CompileErrorKind::UndefinedFunction {
                                name: Arc::clone(&name.name),
                            },
                            (callee.span.start, callee.span.end),
                        ));
                    }
                }
                _ => {
                    // Indirect call through closure.
                    // TODO: Implement CallClosure.
                    return Err(CompileError::new(
                        CompileErrorKind::Unsupported {
                            feature: "indirect function calls".to_string(),
                        },
                        (callee.span.start, callee.span.end),
                    ));
                }
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Abilities
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Perform(ability_call) => {
            compile_ability_call(fc, ability_call, true)?;
        }

        ExprKind::Suspend(ability_call) => {
            compile_ability_call(fc, ability_call, false)?;
        }

        ExprKind::Handle(_handle_expr) => {
            // TODO: Implement handle expression compilation.
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: "handle expressions".to_string(),
                },
                (expr.span.start, expr.span.end),
            ));
        }
    }

    Ok(())
}

/// Compile an ability call (either perform or suspend).
fn compile_ability_call(
    fc: &mut FunctionCompiler,
    ability_call: &crate::ast::AbilityCall,
    perform: bool,
) -> Result<(), CompileError> {
    // Compile arguments.
    for arg in &ability_call.args {
        compile_expr(fc, arg)?;
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

// ─────────────────────────────────────────────────────────────────────────────
// Statement Compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Compile a statement.
fn compile_stmt(fc: &mut FunctionCompiler, stmt: &Stmt) -> Result<(), CompileError> {
    match &stmt.kind {
        StmtKind::Let(let_binding) => {
            compile_let(fc, let_binding)?;
        }
        StmtKind::Expr(expr) => {
            compile_expr(fc, expr)?;
            // Discard the result of expression statements.
            fc.builder.emit(Opcode::Pop);
        }
    }
    Ok(())
}

/// Compile a let binding.
fn compile_let(fc: &mut FunctionCompiler, binding: &LetBinding) -> Result<(), CompileError> {
    // Compile the initializer.
    compile_expr(fc, &binding.init)?;

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
    compile_expr(fc, scrutinee)?;

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
            compile_expr(fc, guard)?;
            if let Some(fj) = fail_jump {
                // If pattern matched but guard fails, need to jump to next arm.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                // Pattern and guard both succeeded - compile body.
                compile_expr(fc, &arm.body)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                fc.builder.patch_jump(fj);
            } else {
                // Last arm with guard.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                compile_expr(fc, &arm.body)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                // If guard fails on last arm, push unit as result.
                fc.builder.emit_const(Value::Unit);
            }
        } else if let Some(fj) = fail_jump {
            // Pattern match can fail, compile body and jump to end.
            compile_expr(fc, &arm.body)?;
            let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
            end_jumps.push(end_jump);
            fc.builder.patch_jump(fj);
        } else {
            // Pattern always matches (wildcard or binding) and it's the last arm.
            compile_expr(fc, &arm.body)?;
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

        PatternKind::Tuple(_) | PatternKind::Record(_) | PatternKind::Variant(_, _) => {
            Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: "complex patterns".to_string(),
                },
                (pattern.span.start, pattern.span.end),
            ))
        }
    }
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
        compile_function_with_hash(func, &hashes)
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
        assert!(compiled.constants.iter().any(|v| matches!(v, Value::Number(n) if (n - 42.0).abs() < f64::EPSILON)));
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

        let func1 = compiled1.get_function("add_one").expect("add_one not found");
        let func2 = compiled2.get_function("increment").expect("increment not found");

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
                                vec![Expr::binary(BinaryOp::Sub, Expr::local(0), Expr::number(1.0))],
                            ),
                        )),
                    ),
                }),
                test_span(),
            )],
        };

        let compiled = compile_module(&module).expect("compilation failed");
        let func = compiled.get_function("factorial").expect("factorial not found");

        // Verify the hash is deterministic - compile again and check
        let compiled2 = compile_module(&module).expect("compilation failed");
        let func2 = compiled2.get_function("factorial").expect("factorial not found");

        assert_eq!(
            func.hash, func2.hash,
            "Recursive function hash should be deterministic"
        );
    }
}
