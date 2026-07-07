//! REPL compilation support.
//!
//! Provides utilities for interactive compilation of expressions and
//! definitions in a REPL environment.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ConstDef, Expr, FunctionDef, ItemKind};
use crate::bytecode::{CompiledFunction, Opcode};
use crate::value::{ModuleExport, ModuleExportKind, ModuleValue};

use super::error::{CompileError, CompileErrorKind};
use super::{FunctionCompiler, ModuleContext, compile_expr, compute_temporary_hash};

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
    /// Available modules for introspection (path -> module value).
    pub modules: HashMap<Arc<str>, Arc<ModuleValue>>,
    /// Prelude abilities (e.g. the platform bindings interface) that
    /// ability calls compile against.
    pub prelude_abilities: Vec<Arc<crate::ability_resolver::DynAbility>>,
}

impl ReplContext {
    /// Create a new empty REPL context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a REPL context with prelude abilities registered.
    #[must_use]
    pub fn with_prelude(prelude: Vec<Arc<crate::ability_resolver::DynAbility>>) -> Self {
        Self {
            prelude_abilities: prelude,
            ..Self::default()
        }
    }

    /// A module context with this REPL's prelude abilities registered.
    fn module_context(&self) -> ModuleContext {
        let mut ctx = ModuleContext::new();
        ctx.register_prelude_abilities(&self.prelude_abilities);
        ctx
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

    /// Register a module for introspection.
    pub fn register_module(&mut self, path: impl Into<Arc<str>>, module: ModuleValue) {
        let path = path.into();
        self.modules.insert(path, Arc::new(module));
    }

    /// Look up a module by path.
    #[must_use]
    pub fn get_module(&self, path: &str) -> Option<&Arc<ModuleValue>> {
        self.modules.get(path)
    }

    /// Check if a path refers to a module.
    #[must_use]
    pub fn is_module(&self, path: &str) -> bool {
        self.modules.contains_key(path)
    }

    /// Look up a module member by path (e.g., `core::collections::List::first`).
    /// Returns the export kind if found.
    #[must_use]
    pub fn get_module_member(&self, path: &str) -> Option<ModuleExportKind> {
        // Split path into module path and member name
        // e.g., "core::collections::List::first" -> module="core::collections::List", member="first"
        let sep_pos = path.rfind("::")?;
        let module_path = &path[..sep_pos];
        let member_name = &path[sep_pos + 2..];

        // Look up the module
        let module = self.modules.get(module_path)?;

        // Look up the member in the module's exports
        module
            .exports
            .iter()
            .find(|e| e.name.as_ref() == member_name)
            .map(|e| e.kind)
    }

    /// Register core library modules with their exports.
    pub fn register_core_modules(&mut self) {
        use crate::compiler::intrinsics::get_intrinsics_for_module;
        use crate::core_library::CoreLibrary;

        // Register the "core" parent module
        let core_modules: Vec<String> = CoreLibrary::available_modules();
        let core_exports: Vec<ModuleExport> = core_modules
            .iter()
            .map(|name| ModuleExport::new(name.clone(), ModuleExportKind::Module))
            .collect();
        self.register_module("core", ModuleValue::new("core", core_exports));

        // Register each core submodule
        for module_name in &core_modules {
            let mut exports = Vec::new();

            // Parse exports from source file
            if let Ok(source) = CoreLibrary::get_source(&[Arc::from(module_name.as_str())]) {
                exports = parse_module_exports(source);
            }

            // Add intrinsics for this module. Module names are fully
            // qualified relative to `core` (`collections::List`), so split
            // them back into path segments for the intrinsic lookup.
            let mut segments: Vec<&str> = vec!["core"];
            segments.extend(module_name.split("::"));
            let intrinsics = get_intrinsics_for_module(&segments);
            for (name, arity) in intrinsics {
                // Only add if not already present from source parsing
                if !exports.iter().any(|e| e.name.as_ref() == name) {
                    // Generate a simple signature based on arity
                    let signature = generate_intrinsic_signature(arity);
                    exports.push(ModuleExport::with_signature(
                        name,
                        ModuleExportKind::Function,
                        signature,
                    ));
                }
            }

            let path = format!("core::{module_name}");
            self.register_module(path.clone(), ModuleValue::new(path, exports));
        }
    }

    /// Register a core library function with its qualified name.
    ///
    /// Called by the REPL initialization code after compiling core library modules.
    /// E.g., `register_core_function("core::collections::List::last", hash)` makes `core::collections::List::last()`
    /// callable from the REPL.
    pub fn register_core_function(&mut self, qualified_name: Arc<str>, hash: blake3::Hash) {
        self.function_hashes.insert(qualified_name.clone(), hash);
        self.item_kinds
            .insert(qualified_name, ReplItemKind::Function);
    }

    /// Register standard abilities as modules for introspection.
    pub fn register_ability_modules(&mut self) {
        // Stdio ability
        self.register_module(
            "Stdio",
            ModuleValue::new(
                "Stdio",
                vec![
                    ModuleExport::new("out!", ModuleExportKind::Function),
                    ModuleExport::new("err!", ModuleExportKind::Function),
                    ModuleExport::new("read!", ModuleExportKind::Function),
                ],
            ),
        );

        // Time ability
        self.register_module(
            "Time",
            ModuleValue::new(
                "Time",
                vec![
                    ModuleExport::new("now!", ModuleExportKind::Function),
                    ModuleExport::new("wait!", ModuleExportKind::Function),
                ],
            ),
        );

        // Random ability
        self.register_module(
            "Random",
            ModuleValue::new(
                "Random",
                vec![
                    ModuleExport::new("random!", ModuleExportKind::Function),
                    ModuleExport::new("random_range!", ModuleExportKind::Function),
                    ModuleExport::new("seed!", ModuleExportKind::Function),
                ],
            ),
        );

        // Log ability
        self.register_module(
            "Log",
            ModuleValue::new(
                "Log",
                vec![
                    ModuleExport::new("info!", ModuleExportKind::Function),
                    ModuleExport::new("debug!", ModuleExportKind::Function),
                    ModuleExport::new("warn!", ModuleExportKind::Function),
                    ModuleExport::new("error!", ModuleExportKind::Function),
                ],
            ),
        );

        // Exception ability
        self.register_module(
            "Exception",
            ModuleValue::new(
                "Exception",
                vec![ModuleExport::new("throw!", ModuleExportKind::Function)],
            ),
        );

        // FileSystem ability
        self.register_module(
            "FileSystem",
            ModuleValue::new(
                "FileSystem",
                vec![
                    ModuleExport::new("read!", ModuleExportKind::Function),
                    ModuleExport::new("write!", ModuleExportKind::Function),
                    ModuleExport::new("read_binary!", ModuleExportKind::Function),
                    ModuleExport::new("write_binary!", ModuleExportKind::Function),
                    ModuleExport::new("exists!", ModuleExportKind::Function),
                    ModuleExport::new("list!", ModuleExportKind::Function),
                    ModuleExport::new("remove!", ModuleExportKind::Function),
                    ModuleExport::new("create_dir!", ModuleExportKind::Function),
                ],
            ),
        );

        // Network ability
        self.register_module(
            "Network",
            ModuleValue::new(
                "Network",
                vec![
                    ModuleExport::new("get!", ModuleExportKind::Function),
                    ModuleExport::new("post!", ModuleExportKind::Function),
                ],
            ),
        );
    }
}

/// Parse module exports from source code using simple pattern matching.
/// This is a lightweight parser that extracts pub fn, const, type, and enum declarations.
#[must_use]
pub fn parse_module_exports(source: &str) -> Vec<ModuleExport> {
    let mut exports = Vec::new();

    for line in source.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.starts_with("//") || line.is_empty() {
            continue;
        }

        // A nominal `type`/`enum` carries a `unique(<uuid>)` prefix (after an
        // optional `pub`). Normalize it away so the keyword match below sees a
        // bare `type ...` / `enum ...`, the same as a non-nominal declaration.
        let decl = strip_unique_prefix(line.strip_prefix("pub ").unwrap_or(line));

        // Match pub fn declarations (public functions)
        if let Some(rest) = line.strip_prefix("pub fn ") {
            if let Some((name, signature)) = extract_function_signature(rest) {
                exports.push(ModuleExport::with_signature(
                    name,
                    ModuleExportKind::Function,
                    signature,
                ));
            }
        }
        // Match fn declarations (private - skip for exports but include for now)
        else if let Some(rest) = line.strip_prefix("fn ") {
            // Skip private helper functions (those with underscores or _helper suffix)
            if let Some((name, _)) = extract_function_signature(rest) {
                if name.contains("_helper") || name.starts_with('_') {
                    continue;
                }
                // Include non-helper functions with signatures
                if let Some((name, signature)) = extract_function_signature(rest) {
                    exports.push(ModuleExport::with_signature(
                        name,
                        ModuleExportKind::Function,
                        signature,
                    ));
                }
            }
        }
        // Match const declarations (they're implicitly public in Ambient)
        else if let Some(rest) = line.strip_prefix("const ") {
            if let Some((name, type_str)) = extract_const_signature(rest) {
                if let Some(type_str) = type_str {
                    exports.push(ModuleExport::with_signature(
                        name,
                        ModuleExportKind::Const,
                        type_str,
                    ));
                } else {
                    exports.push(ModuleExport::new(name, ModuleExportKind::Const));
                }
            }
        }
        // Match struct declarations (bare or `unique(...) struct`)
        else if let Some(rest) = decl.strip_prefix("struct ") {
            if let Some(name) = extract_identifier(rest) {
                exports.push(ModuleExport::new(name, ModuleExportKind::Type));
            }
        }
        // Match type alias declarations
        else if let Some(rest) = decl.strip_prefix("type ") {
            if let Some(name) = extract_identifier(rest) {
                exports.push(ModuleExport::new(name, ModuleExportKind::Type));
            }
        }
        // Match enum declarations (always `unique(...) enum`)
        else if let Some(rest) = decl.strip_prefix("enum ") {
            if let Some(name) = extract_identifier(rest) {
                exports.push(ModuleExport::new(name, ModuleExportKind::Enum));
            }
        }
        // Match ability declarations
        else if let Some(rest) = line
            .strip_prefix("pub ability ")
            .or_else(|| line.strip_prefix("ability "))
            && let Some(name) = extract_identifier(rest)
        {
            exports.push(ModuleExport::new(name, ModuleExportKind::Ability));
        }
    }

    exports
}

/// Strip a leading `unique(<uuid>)` prefix (plus trailing whitespace) from a
/// declaration line. Returns the line unchanged when there is no such prefix.
/// This lets the REPL's line scanner treat nominal `type`/`enum` declarations
/// the same as bare ones.
fn strip_unique_prefix(line: &str) -> &str {
    let Some(after) = line.strip_prefix("unique(") else {
        return line;
    };
    match after.find(')') {
        Some(idx) => after[idx + 1..].trim_start(),
        None => line,
    }
}

/// Extract an identifier from the start of a string.
fn extract_identifier(s: &str) -> Option<String> {
    let s = s.trim();
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if end > 0 {
        Some(s[..end].to_string())
    } else {
        None
    }
}

/// Extract function name and signature from a function declaration.
/// Input: "map(xs: List<a>, f: (a) -> b): List<b> {"
/// Output: ("map", "(xs: List<a>, f: (a) -> b): List<b>")
fn extract_function_signature(s: &str) -> Option<(String, String)> {
    let s = s.trim();

    // Find the function name (identifier before '(' or '<')
    let name_end = s.find(['(', '<']).unwrap_or(s.len());
    let name = s[..name_end].trim().to_string();
    if name.is_empty() {
        return None;
    }

    // Find the signature: from first '(' to before '{'
    let sig_start = s.find('(')?;
    let sig_end = find_signature_end(s, sig_start);
    let mut signature = s[sig_start..sig_end].trim().to_string();

    // Remove trailing '{' if present
    if signature.ends_with('{') {
        signature = signature.trim_end_matches('{').trim().to_string();
    }

    Some((name, signature))
}

/// Find the end of a function signature (just before the opening brace).
/// Handles nested parentheses and angle brackets for generic types.
fn find_signature_end(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = start;
    let mut paren_depth = 0;
    let mut angle_depth = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'<' => angle_depth += 1,
            b'>' => angle_depth -= 1,
            b'{' if paren_depth == 0 && angle_depth == 0 => {
                // Found the opening brace, signature ends here
                return i;
            }
            _ => {}
        }
        i += 1;
    }

    // No opening brace found, return end of string
    s.len()
}

/// Generate a simple signature for an intrinsic based on arity.
fn generate_intrinsic_signature(arity: u8) -> String {
    let params: Vec<String> = (0..arity).map(|i| format!("arg{i}")).collect();
    format!("({})", params.join(", "))
}

/// Extract constant name and optional type from a const declaration.
/// Input: "PI: number = 3.14159"
/// Output: ("PI", Some("number"))
fn extract_const_signature(s: &str) -> Option<(String, Option<String>)> {
    let s = s.trim();

    // Find the constant name
    let name_end = s.find([':', '='])?;
    let name = s[..name_end].trim().to_string();
    if name.is_empty() {
        return None;
    }

    // Check if there's a type annotation
    if s.as_bytes().get(name_end) == Some(&b':') {
        // Find the type: from ':' to '='
        let type_start = name_end + 1;
        let type_end = s[type_start..].find('=').map(|i| type_start + i)?;
        let type_str = s[type_start..type_end].trim().to_string();
        if type_str.is_empty() {
            Some((name, None))
        } else {
            Some((name, Some(type_str)))
        }
    } else {
        Some((name, None))
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
    let mut ctx = context.module_context();

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

            let mut module_ctx = context.module_context();
            let compiled = compile_function_for_repl(func, &hashes, &mut module_ctx, context)?;

            Ok(CompiledReplItem {
                name: Arc::clone(&func.name),
                function: compiled,
                kind: ReplItemKind::Function,
            })
        }
        ItemKind::Const(const_def) => {
            let hashes = context.function_hashes.clone();
            let mut module_ctx = context.module_context();
            let compiled = compile_const_for_repl(const_def, &hashes, &mut module_ctx, context)?;

            Ok(CompiledReplItem {
                name: Arc::clone(&const_def.name),
                function: compiled,
                kind: ReplItemKind::Constant,
            })
        }
        ItemKind::Struct(_)
        | ItemKind::TypeAlias(_)
        | ItemKind::Enum(_)
        | ItemKind::Ability(_)
        | ItemKind::Use(_)
        | ItemKind::Trait(_)
        | ItemKind::Impl(_) => Err(CompileError::new(
            CompileErrorKind::Internal {
                message: "structs, type aliases, enums, abilities, traits, impls, and use statements are not yet supported in the REPL",
            },
            (item.span.start, item.span.end),
        )),
    }
}
