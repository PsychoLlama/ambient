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
    let mut result = CompiledModule::new();

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

    // For proper handling of recursive calls, we need to compile functions
    // using a stable hash that doesn't depend on call targets.
    // For now, use a simpler approach: compile with name-based lookup at runtime.
    //
    // Phase 1: Create stable hashes based on function structure (not call targets).
    let mut function_names: HashMap<Arc<str>, blake3::Hash> = HashMap::new();
    for func in &functions {
        // Create a deterministic hash based on function structure.
        // This includes: name, params, body structure (but not call target hashes).
        let hash = compute_function_hash(func);
        function_names.insert(Arc::clone(&func.name), hash);
    }

    // Phase 2: Compile each function using the pre-computed hashes.
    for func in &functions {
        let compiled = compile_function_with_hash(func, &function_names)?;
        let hash = function_names[&func.name];

        result.functions.insert(hash, compiled);
        result.function_names.insert(Arc::clone(&func.name), hash);

        // Check for main function.
        if &*func.name == "main" {
            result.entry_point = Some(hash);
        }
    }

    // Compile constants.
    for item in &module.items {
        if let ItemKind::Const(const_def) = &item.kind {
            let compiled = compile_const(const_def, &result.function_names)?;
            let hash = compiled.hash;
            result.functions.insert(hash, compiled);
            result
                .function_names
                .insert(Arc::clone(&const_def.name), hash);
        }
    }

    Ok(result)
}

/// Compute a stable hash for a function based on its structure.
/// This hash is deterministic and doesn't depend on call target hashes,
/// allowing for recursive functions to be properly identified.
fn compute_function_hash(func: &FunctionDef) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();

    // Hash function name
    hasher.update(func.name.as_bytes());

    // Hash parameter count and names
    hasher.update(&(func.params.len() as u32).to_le_bytes());
    for param in &func.params {
        hasher.update(param.name.as_bytes());
    }

    // Hash public flag
    hasher.update(&[u8::from(func.is_public)]);

    // Hash a structural representation of the body
    // For now, just hash the span as a proxy for body structure
    hasher.update(&func.body.span.start.to_le_bytes());
    hasher.update(&func.body.span.end.to_le_bytes());

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
        let hash = compute_function_hash(func);
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
}
