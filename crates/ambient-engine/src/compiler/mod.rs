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
mod intrinsics;
mod lambdas;
mod patterns;
mod repl;

pub use error::{CompileError, CompileErrorKind};
pub use repl::{
    compile_expression, compile_expression_with_context, compile_repl_item, parse_module_exports,
    CompiledReplItem, ReplContext, ReplItemKind,
};

// Re-export for use by submodules
use expr::compile_expr;
use hash::{compute_temporary_hash, finalize_module_hashes};

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{BindingId, ConstDef, FunctionDef, ItemKind, Module};
use crate::bytecode::{BytecodeBuilder, CompiledFunction, DebugInfo, Opcode};
use crate::value::Value;

// ─────────────────────────────────────────────────────────────────────────────
// Handler implicit parameter names
// ─────────────────────────────────────────────────────────────────────────────

/// Name for the implicit continuation parameter in handler functions (slot 0).
const HANDLER_PARAM_CONTINUATION: &str = "__continuation";

/// Name for the implicit suspended ability parameter in handler functions (slot 1).
const HANDLER_PARAM_SUSPENDED_ABILITY: &str = "__suspended_ability";

/// A compiled function entry with metadata for hash finalization.
///
/// Fields: (name, function, `is_main`, `lambda_parent`)
/// - `name`: Function name or synthetic lambda key
/// - `function`: The compiled function
/// - `is_main`: Whether this is the module entry point
/// - `lambda_parent`: If Some, this is a lambda and contains the parent function name
type FunctionEntry = (Arc<str>, CompiledFunction, bool, Option<Arc<str>>);

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

    /// Map from lambda hashes to their parent function names.
    /// Used for navigation: to find a lambda's source location,
    /// compile the parent and match by hash.
    pub lambda_parents: HashMap<blake3::Hash, Arc<str>>,

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
            lambda_parents: HashMap::new(),
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
        for (hash, parent) in &other.lambda_parents {
            self.lambda_parents
                .entry(*hash)
                .or_insert_with(|| Arc::clone(parent));
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
    /// Maps (temporary hash, parent function name) to compiled function.
    lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)>,
    /// Counter for generating unique lambda temporary hashes.
    lambda_counter: u32,
    /// Name of the function currently being compiled.
    /// Used to track lambda parent relationships.
    current_function: Option<Arc<str>>,
}

impl ModuleContext {
    fn new() -> Self {
        Self {
            lambdas: Vec::new(),
            lambda_counter: 0,
            current_function: None,
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
        // Track current function for lambda parent relationships.
        ctx.set_current_function(Arc::clone(&func.name));
        let compiled =
            compile_function_with_hash(func, &temp_hashes, &mut ctx, source, source_file)?;
        let is_main = &*func.name == "run";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Compile constants.
    for item in &module.items {
        if let ItemKind::Const(const_def) = &item.kind {
            // Track current function for lambda parent relationships.
            ctx.set_current_function(Arc::clone(&const_def.name));
            let compiled = compile_const(const_def, &temp_hashes, &mut ctx, source, source_file)?;
            compiled_functions.push((Arc::clone(&const_def.name), compiled, false));
        }
    }

    // Collect lambda info: (temp_hash, parent_name, compiled_func)
    // We need temp hashes for the call graph, but lambdas go to lambda_parents not function_names.
    let lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)> = ctx.lambdas;
    for (lambda_hash, _parent, _func) in &lambdas {
        // Add to temp_hashes so call graph analysis works.
        // Use hash as "name" for the temp mapping.
        let lambda_key: Arc<str> = format!("__lambda_{lambda_hash}").into();
        temp_hashes.insert(lambda_key, *lambda_hash);
    }

    // Phase 3: Compute content-addressed hashes and finalize the module.
    finalize_module_hashes(compiled_functions, lambdas, &temp_hashes)
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
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Expr, ExprKind, FunctionDef, Item, Param, Span};

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
            doc: None,
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
            doc: None,
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
            doc: None,
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
            doc: None,
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
            doc: None,
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
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(Expr::local(0)),
                    right: Box::new(Expr::local(1)),
                    resolved_op: None,
                },
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

    // ─────────────────────────────────────────────────────────────────────────
    // Additional Expression Compilation Tests (Ticket 5.1)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_compile_unit_literal() {
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        // Unit is compiled to Value::Unit constant
        assert!(compiled.constants.iter().any(|v| matches!(v, Value::Unit)));
    }

    #[test]
    fn test_compile_bool_literals() {
        // Test true
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::bool(true),
        };
        let compiled = compile_test_function(&func).expect("compilation failed");
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(true))));

        // Test false
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::bool(false),
        };
        let compiled = compile_test_function(&func).expect("compilation failed");
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(false))));
    }

    #[test]
    fn test_compile_string_literal() {
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::string("hello world"),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::String(s) if s.as_ref() == "hello world")));
    }

    #[test]
    fn test_compile_tuple() {
        // fn test() { (1, "hello", true) }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::tuple(vec![
                Expr::number(1.0),
                Expr::string("hello"),
                Expr::bool(true),
            ]),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        // Should have all three constants
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Number(n) if (n - 1.0).abs() < f64::EPSILON)));
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::String(s) if s.as_ref() == "hello")));
        assert!(compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(true))));
    }

    #[test]
    fn test_compile_tuple_index() {
        // fn test(t) { t.0 }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "t")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::tuple_index(Expr::local(0), 0),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_record() {
        // fn test() { { x: 1, y: 2 } }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        // Should have both number constants
        let number_count = compiled
            .constants
            .iter()
            .filter(|v| matches!(v, Value::Number(_)))
            .count();
        assert!(number_count >= 2);
    }

    #[test]
    fn test_compile_record_field() {
        // fn test(r) { r.x }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "r")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::field_access(Expr::local(0), "x"),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_list() {
        // fn test() { [1, 2, 3] }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::new(
                ExprKind::List(vec![
                    Expr::number(1.0),
                    Expr::number(2.0),
                    Expr::number(3.0),
                ]),
                Span::default(),
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        // Should have all three number constants
        let number_count = compiled
            .constants
            .iter()
            .filter(|v| matches!(v, Value::Number(_)))
            .count();
        assert!(number_count >= 3);
    }

    #[test]
    fn test_compile_unary_neg() {
        // fn test(x) { -x }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unary(crate::ast::UnaryOp::Neg, Expr::local(0)),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_unary_not() {
        // fn test(x) { !x }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unary(crate::ast::UnaryOp::Not, Expr::local(0)),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_binary_comparison() {
        // fn test(x, y) { x == y }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x"), Param::new(1, "y")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::local(1)),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 2);
    }

    #[test]
    fn test_compile_binary_logical() {
        // fn test(a, b) { a && b }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "a"), Param::new(1, "b")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::binary(BinaryOp::And, Expr::local(0), Expr::local(1)),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 2);
    }

    #[test]
    fn test_compile_block() {
        use crate::ast::{LetBinding, Stmt, StmtKind};

        // fn test() { let x = 1; let y = 2; x + y }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::block(
                vec![
                    Stmt::new(
                        StmtKind::Let(LetBinding {
                            id: 0,
                            name: Arc::from("x"),
                            ty: None,
                            init: Expr::number(1.0),
                        }),
                        Span::default(),
                    ),
                    Stmt::new(
                        StmtKind::Let(LetBinding {
                            id: 1,
                            name: Arc::from("y"),
                            ty: None,
                            init: Expr::number(2.0),
                        }),
                        Span::default(),
                    ),
                ],
                Some(Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1))),
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert!(compiled.local_count >= 2);
    }

    #[test]
    fn test_compile_lambda() {
        // fn test() { (x) => x + 1 }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::lambda(
                vec![Param::new(0, "x")],
                Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
            ),
        };

        // Lambda compilation should succeed
        let _compiled = compile_test_function(&func).expect("compilation failed");
    }

    #[test]
    fn test_compile_closure_capture() {
        use crate::ast::{LetBinding, Stmt, StmtKind};

        // fn test() { let y = 10; (x) => x + y }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::block(
                vec![Stmt::new(
                    StmtKind::Let(LetBinding {
                        id: 0,
                        name: Arc::from("y"),
                        ty: None,
                        init: Expr::number(10.0),
                    }),
                    Span::default(),
                )],
                Some(Expr::lambda(
                    vec![Param::new(1, "x")],
                    Expr::binary(BinaryOp::Add, Expr::local(1), Expr::local(0)),
                )),
            ),
        };

        // Closure capturing outer variable should compile
        let _compiled = compile_test_function(&func).expect("compilation failed");
    }

    #[test]
    fn test_compile_if_without_else() {
        // fn test(x) { if x { 1 } } - returns unit when else omitted
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::if_then_else(Expr::local(0), Expr::number(1.0), None),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_nested_if() {
        // fn test(a, b) { if a { if b { 1 } else { 2 } } else { 3 } }
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "a"), Param::new(1, "b")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::if_then_else(
                Expr::local(0),
                Expr::if_then_else(Expr::local(1), Expr::number(1.0), Some(Expr::number(2.0))),
                Some(Expr::number(3.0)),
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 2);
    }

    #[test]
    fn test_compile_all_arithmetic_ops() {
        // Test all arithmetic binary operators
        for op in [
            BinaryOp::Add,
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Mod,
        ] {
            let func = FunctionDef {
                name: Arc::from("test"),
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::new(0, "x"), Param::new(1, "y")],
                ret_ty: None,
                abilities: vec![],
                body: Expr::binary(op, Expr::local(0), Expr::local(1)),
            };

            let compiled =
                compile_test_function(&func).expect(&format!("{op:?} compilation failed"));
            assert_eq!(compiled.param_count, 2);
        }
    }

    #[test]
    fn test_compile_all_comparison_ops() {
        // Test all comparison binary operators
        for op in [
            BinaryOp::Eq,
            BinaryOp::Ne,
            BinaryOp::Lt,
            BinaryOp::Le,
            BinaryOp::Gt,
            BinaryOp::Ge,
        ] {
            let func = FunctionDef {
                name: Arc::from("test"),
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::new(0, "x"), Param::new(1, "y")],
                ret_ty: None,
                abilities: vec![],
                body: Expr::binary(op, Expr::local(0), Expr::local(1)),
            };

            let compiled =
                compile_test_function(&func).expect(&format!("{op:?} compilation failed"));
            assert_eq!(compiled.param_count, 2);
        }
    }

    #[test]
    fn test_compile_match_literal_pattern() {
        use crate::ast::{Literal, MatchArm, Pattern};

        // fn test(x) { match x { 42 => true, _ => false } }
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
                    MatchArm::new(Pattern::literal(Literal::Number(42.0)), Expr::bool(true)),
                    MatchArm::new(Pattern::wildcard(), Expr::bool(false)),
                ],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
    }

    #[test]
    fn test_compile_match_binding_pattern() {
        use crate::ast::{MatchArm, Pattern};

        // fn test(x) { match x { y => y + 1 } }
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
                vec![MatchArm::new(
                    Pattern::binding(1, "y"),
                    Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
                )],
            ),
        };

        let compiled = compile_test_function(&func).expect("compilation failed");
        assert_eq!(compiled.param_count, 1);
        assert!(compiled.local_count >= 2); // x and y
    }

    #[test]
    fn test_compile_match_tuple_pattern_unsupported() {
        use crate::ast::{MatchArm, Pattern, PatternKind};

        // Tuple patterns are not yet supported, so this should return an error
        // fn test(t) { match t { (a, b) => a + b } }
        let tuple_pat = Pattern::new(
            PatternKind::Tuple(vec![Pattern::binding(1, "a"), Pattern::binding(2, "b")]),
            Span::default(),
        );
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "t")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::match_expr(
                Expr::local(0),
                vec![MatchArm::new(
                    tuple_pat,
                    Expr::binary(BinaryOp::Add, Expr::local(1), Expr::local(2)),
                )],
            ),
        };

        // Tuple patterns are not yet supported - expect an error
        let result = compile_test_function(&func);
        assert!(result.is_err(), "Tuple patterns should be unsupported");
    }

    #[test]
    fn test_compile_nested_function_calls() {
        // fn test() { add(mul(2, 3), mul(4, 5)) }
        let module = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("mul"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "x"), Param::new(1, "y")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(BinaryOp::Mul, Expr::local(0), Expr::local(1)),
                    }),
                    test_span(),
                ),
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("add"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "x"), Param::new(1, "y")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
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
                        body: Expr::call(
                            Expr::name("add"),
                            vec![
                                Expr::call(
                                    Expr::name("mul"),
                                    vec![Expr::number(2.0), Expr::number(3.0)],
                                ),
                                Expr::call(
                                    Expr::name("mul"),
                                    vec![Expr::number(4.0), Expr::number(5.0)],
                                ),
                            ],
                        ),
                    }),
                    test_span(),
                ),
            ],
        };

        let compiled = compile_module(&module).expect("compilation failed");
        assert!(compiled.get_function("run").is_some());
        assert!(compiled.get_function("add").is_some());
        assert!(compiled.get_function("mul").is_some());
    }

    #[test]
    fn test_compile_mutual_recursion() {
        // fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
        // fn is_odd(n) { if n == 0 { false } else { is_even(n - 1) } }
        let module = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("is_even"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "n")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::if_then_else(
                            Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::number(0.0)),
                            Expr::bool(true),
                            Some(Expr::call(
                                Expr::name("is_odd"),
                                vec![Expr::binary(
                                    BinaryOp::Sub,
                                    Expr::local(0),
                                    Expr::number(1.0),
                                )],
                            )),
                        ),
                    }),
                    test_span(),
                ),
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("is_odd"),
                        name_span: Span::default(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![Param::new(0, "n")],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::if_then_else(
                            Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::number(0.0)),
                            Expr::bool(false),
                            Some(Expr::call(
                                Expr::name("is_even"),
                                vec![Expr::binary(
                                    BinaryOp::Sub,
                                    Expr::local(0),
                                    Expr::number(1.0),
                                )],
                            )),
                        ),
                    }),
                    test_span(),
                ),
            ],
        };

        let compiled = compile_module(&module).expect("compilation failed");
        assert!(compiled.get_function("is_even").is_some());
        assert!(compiled.get_function("is_odd").is_some());
    }
}
