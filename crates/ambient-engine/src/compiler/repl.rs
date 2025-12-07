//! REPL compilation support.
//!
//! Provides utilities for interactive compilation of expressions and
//! definitions in a REPL environment.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ConstDef, Expr, FunctionDef, ItemKind};
use crate::bytecode::{CompiledFunction, Opcode};

use super::error::{CompileError, CompileErrorKind};
use super::{compile_expr, compute_temporary_hash, FunctionCompiler, ModuleContext};

/// Whether a name refers to a function or a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplItemKind {
    /// A function that can be called with arguments.
    Function,
    /// A constant (thunk) that should be auto-evaluated when referenced.
    Constant,
}

/// Context for REPL compilation, tracking defined names across evaluations.
#[derive(Debug, Clone, Default)]
pub struct ReplContext {
    /// Map from function/constant names to their hashes.
    pub function_hashes: HashMap<Arc<str>, blake3::Hash>,
    /// Map from names to their kinds (function or constant).
    pub item_kinds: HashMap<Arc<str>, ReplItemKind>,
}

impl ReplContext {
    /// Create a new empty REPL context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a function with its hash.
    pub fn register_function(&mut self, name: Arc<str>, hash: blake3::Hash) {
        self.function_hashes.insert(name.clone(), hash);
        self.item_kinds.insert(name, ReplItemKind::Function);
    }

    /// Register a constant with its hash.
    pub fn register_constant(&mut self, name: Arc<str>, hash: blake3::Hash) {
        self.function_hashes.insert(name.clone(), hash);
        self.item_kinds.insert(name, ReplItemKind::Constant);
    }

    /// Check if a name refers to a constant.
    #[must_use]
    pub fn is_constant(&self, name: &str) -> bool {
        self.item_kinds
            .get(name)
            .is_some_and(|k| *k == ReplItemKind::Constant)
    }
}

/// Result of compiling a REPL item (function or constant).
#[derive(Debug)]
pub struct CompiledReplItem {
    /// The name of the defined item.
    pub name: Arc<str>,
    /// The compiled function.
    pub function: CompiledFunction,
    /// The kind of item (function or constant).
    pub kind: ReplItemKind,
}

/// Compile a standalone expression for REPL evaluation.
///
/// This wraps the expression in an anonymous function and compiles it.
/// The function takes no parameters and returns the expression's value.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_expression(expr: &Expr) -> Result<CompiledFunction, CompileError> {
    compile_expression_with_context(expr, &ReplContext::new())
}

/// Compile a standalone expression for REPL evaluation with context.
///
/// This wraps the expression in an anonymous function and compiles it.
/// The function takes no parameters and returns the expression's value.
/// The context provides access to previously defined functions and constants.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_expression_with_context(
    expr: &Expr,
    context: &ReplContext,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(context.function_hashes.clone());
    fc.set_repl_context(context);
    let mut ctx = ModuleContext::new();

    // Compile the expression (leaves result on stack).
    compile_expr(&mut fc, expr, &mut ctx)?;

    // Emit return instruction.
    fc.builder.emit(Opcode::Return);

    // Build with no parameters.
    Ok(fc.builder.build(fc.next_local, 0))
}

/// Compile a function for REPL, with access to previously defined constants.
fn compile_function_for_repl(
    func: &FunctionDef,
    function_hashes: &HashMap<Arc<str>, blake3::Hash>,
    ctx: &mut ModuleContext,
    repl_ctx: &ReplContext,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());
    fc.set_repl_context(repl_ctx);

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

    Ok(CompiledFunction {
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info: None,
    })
}

/// Compile a constant for REPL, with access to previously defined constants.
fn compile_const_for_repl(
    const_def: &ConstDef,
    function_hashes: &HashMap<Arc<str>, blake3::Hash>,
    ctx: &mut ModuleContext,
    repl_ctx: &ReplContext,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());
    fc.set_repl_context(repl_ctx);

    // Compile the value expression.
    compile_expr(&mut fc, &const_def.value, ctx)?;

    // Return the value.
    fc.builder.emit(Opcode::Return);

    Ok(fc.builder.build(fc.next_local, 0))
}

/// Compile a single item (function or constant) for the REPL.
///
/// Returns the compiled function along with its name. The caller should
/// register the name and hash in the `ReplContext` after loading the function.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_repl_item(
    item: &crate::ast::Item,
    context: &ReplContext,
) -> Result<CompiledReplItem, CompileError> {
    match &item.kind {
        ItemKind::Function(func) => {
            // Create a hash table including this function for self-recursion.
            let mut hashes = context.function_hashes.clone();
            let temp_hash = compute_temporary_hash(&func.name);
            hashes.insert(Arc::clone(&func.name), temp_hash);

            let mut module_ctx = ModuleContext::new();
            let compiled =
                compile_function_for_repl(func, &hashes, &mut module_ctx, context)?;

            Ok(CompiledReplItem {
                name: Arc::clone(&func.name),
                function: compiled,
                kind: ReplItemKind::Function,
            })
        }
        ItemKind::Const(const_def) => {
            let hashes = context.function_hashes.clone();
            let mut module_ctx = ModuleContext::new();
            let compiled = compile_const_for_repl(const_def, &hashes, &mut module_ctx, context)?;

            Ok(CompiledReplItem {
                name: Arc::clone(&const_def.name),
                function: compiled,
                kind: ReplItemKind::Constant,
            })
        }
        ItemKind::TypeAlias(_) | ItemKind::Enum(_) | ItemKind::Ability(_) | ItemKind::Use(_) => {
            Err(CompileError::new(
                CompileErrorKind::Internal {
                    message: "type aliases, enums, abilities, and use statements are not yet supported in the REPL",
                },
                (item.span.start, item.span.end),
            ))
        }
    }
}
