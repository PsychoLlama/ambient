//! Code completion support for the Ambient language.
//!
//! Provides auto-completion for:
//! - Keywords (fn, let, if, etc.)
//! - Built-in types (number, string, bool)
//! - Function names from the current module
//! - Local variables in scope
//! - Ability names and methods
//! - Core library modules and functions

use ambient_engine::ast::{Expr, ExprKind, FunctionDef, ItemKind, Module, Param, StmtKind};
use ambient_engine::core_library::CoreLibrary;
use ambient_engine::symbol_db::{SymbolDb, SymbolKind};
use ambient_parser::TokenKind;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, InsertTextFormat};
use uuid::Uuid;

use crate::analysis::format_type;

/// A completion context containing information about the cursor position.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct CompletionContext<'a> {
    /// The byte offset of the cursor.
    pub offset: usize,
    /// The word being typed (prefix for filtering).
    pub word_prefix: &'a str,
    /// Whether we're after a dot (field/method access).
    pub after_dot: bool,
    /// Whether we're after an ability name (for method completion).
    pub after_ability_dot: Option<&'a str>,
    /// Whether we're after `core.` (for core library module completion).
    pub after_core_dot: bool,
    /// Whether we're after `core.<submodule>.` (for core submodule member completion).
    /// Contains the submodule name (e.g., "List" for "core.List.").
    pub after_core_submodule_dot: Option<&'a str>,
    /// Whether we're after a pkg module path (for pkg module member completion).
    /// Contains the module path (e.g., "utils" for "utils." or "utils.format" for "utils.format.").
    pub after_pkg_module_dot: Option<&'a str>,
    /// Whether we're after a use statement prefix (pkg, core, self, super).
    pub in_use_statement: bool,
    /// Whether we're inside `unique(` for nominal type UUID completion.
    pub in_unique_paren: bool,
}

impl<'a> CompletionContext<'a> {
    /// Create a completion context from source and offset.
    #[must_use]
    pub fn new(source: &'a str, offset: usize) -> Self {
        let offset = offset.min(source.len());

        // Find the start of the current line.
        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_prefix = &source[line_start..offset];

        // Check if we're in a use statement
        let trimmed_line = line_prefix.trim_start();
        let in_use_statement = trimmed_line.starts_with("use ");

        // Find the word being typed.
        let word_start = line_prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map_or(0, |i| i + 1);
        let word_prefix = &line_prefix[word_start..];

        // Namespace / path access uses `::`; value field access uses `.`.
        let before_word = &line_prefix[..word_start];
        let trimmed_before = before_word.trim_end();
        let after_dot = trimmed_before.ends_with('.');
        let after_scope = trimmed_before.ends_with("::");

        // The qualified path immediately before a trailing `::`
        // (e.g. `core`, `core::List`, `platform::Console`).
        let scope_path = if after_scope {
            let without_sep = trimmed_before.strip_suffix("::").unwrap_or(trimmed_before);
            let start = without_sep
                .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
                .map_or(0, |i| i + 1);
            Some(&without_sep[start..])
        } else {
            None
        };

        // Core library completions: `core::` offers submodules; `core::List::`
        // offers that submodule's members.
        let (after_core_dot, after_core_submodule_dot) = match scope_path {
            Some("core") => (true, None),
            Some(path) => match path.strip_prefix("core::") {
                Some(sub)
                    if !sub.contains("::") && CoreLibrary::available_modules().contains(&sub) =>
                {
                    (false, Some(sub))
                }
                _ => (false, None),
            },
            None => (false, None),
        };

        // Ability method completion (`Console::`, `platform::Console::`) and
        // pkg module member completion (`utils::`).
        let (after_ability_dot, after_pkg_module_dot) = match scope_path {
            Some(path) if !after_core_dot && after_core_submodule_dot.is_none() => {
                let last = path.rsplit("::").next().unwrap_or(path);
                if TokenKind::builtin_abilities().contains(&last) {
                    (Some(last), None)
                } else if path.chars().next().is_some_and(char::is_lowercase)
                    || CoreLibrary::available_modules().contains(&last)
                {
                    // A module path: either a conventional lowercase namespace,
                    // or an alias of a known core module. The latter matters for
                    // the PascalCase type-companion modules (`List`, `Option`,
                    // `Result`): after `use core::List;`, a bare `List::` would
                    // otherwise be misread as an ability and lose completions.
                    (None, Some(path))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        };

        // Check if we're inside `unique(` for nominal type UUID completion.
        // Look for "unique(" before the cursor on the current line, with no closing paren.
        let in_unique_paren = if let Some(unique_pos) = line_prefix.rfind("unique(") {
            // Check there's no closing paren after the opening paren
            let after_open = &line_prefix[unique_pos + 7..];
            !after_open.contains(')')
        } else {
            false
        };

        Self {
            offset,
            word_prefix,
            after_dot,
            after_ability_dot,
            after_core_dot,
            after_core_submodule_dot,
            after_pkg_module_dot,
            in_use_statement,
            in_unique_paren,
        }
    }
}

/// Generate completions for a given context.
#[must_use]
pub fn get_completions(
    ctx: &CompletionContext<'_>,
    module: Option<&Module>,
    symbol_db: Option<&SymbolDb>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // If we're inside unique(), offer a generated UUID
    if ctx.in_unique_paren {
        items.push(get_uuid_completion());
        return items;
    }

    // If we're completing core library modules (after "core.")
    if ctx.after_core_dot {
        items.extend(get_core_module_completions(ctx.word_prefix));
        return items;
    }

    // If we're completing core submodule members (after "core.<submodule>.")
    if let Some(submodule) = ctx.after_core_submodule_dot {
        items.extend(get_core_submodule_member_completions(
            submodule,
            ctx.word_prefix,
        ));
        return items;
    }

    // If we're completing an ability method, show ability methods.
    if let Some(ability_name) = ctx.after_ability_dot {
        items.extend(get_ability_method_completions(
            ability_name,
            ctx.word_prefix,
        ));
        return items;
    }

    // If we're completing pkg module members (after "module_name.")
    if let Some(module_path) = ctx.after_pkg_module_dot {
        if let Some(db) = symbol_db {
            items.extend(get_pkg_module_completions(db, module_path, ctx.word_prefix));
        }
        return items;
    }

    // If we're after a dot (but not an ability, core, or module), we'd show field completions.
    // For now, we don't have enough type info at the cursor, so skip.
    if ctx.after_dot {
        return items;
    }

    // If we're in a use statement, add use prefix completions
    if ctx.in_use_statement {
        items.extend(get_use_prefix_completions(ctx.word_prefix));
    }

    // Add keyword completions.
    items.extend(get_keyword_completions(ctx.word_prefix));

    // Add builtin type completions.
    items.extend(get_type_completions(ctx.word_prefix));

    // Add ability completions.
    items.extend(get_ability_completions(ctx.word_prefix));

    // Add function name completions from the module.
    if let Some(module) = module {
        items.extend(get_function_completions(module, ctx.word_prefix));
        items.extend(get_local_completions(module, ctx.offset, ctx.word_prefix));
    }

    items
}

/// Generate a completion item with a fresh UUID for nominal type definitions.
fn get_uuid_completion() -> CompletionItem {
    let uuid = Uuid::new_v4().to_string();
    CompletionItem {
        label: uuid.clone(),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some("Generated UUID for nominal type".to_string()),
        insert_text: Some(uuid),
        ..Default::default()
    }
}

/// Get keyword completions.
fn get_keyword_completions(prefix: &str) -> Vec<CompletionItem> {
    TokenKind::all_keywords()
        .iter()
        .filter(|kw| kw.starts_with(prefix))
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("keyword".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get built-in type completions.
fn get_type_completions(prefix: &str) -> Vec<CompletionItem> {
    TokenKind::builtin_types()
        .iter()
        .filter(|ty| ty.starts_with(prefix))
        .map(|ty| CompletionItem {
            label: (*ty).to_string(),
            kind: Some(CompletionItemKind::TYPE_PARAMETER),
            detail: Some("type".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get ability completions.
fn get_ability_completions(prefix: &str) -> Vec<CompletionItem> {
    TokenKind::builtin_abilities()
        .iter()
        .filter(|ab| ab.starts_with(prefix))
        .map(|ab| CompletionItem {
            label: (*ab).to_string(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some("ability".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get core library module completions.
fn get_core_module_completions(prefix: &str) -> Vec<CompletionItem> {
    CoreLibrary::available_modules()
        .into_iter()
        .filter(|module| module.starts_with(prefix))
        .map(|module| CompletionItem {
            label: module.to_string(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(format!("core library module: {module}")),
            documentation: Some(lsp_types::Documentation::String(get_core_module_doc(
                module,
            ))),
            ..Default::default()
        })
        .collect()
}

/// Get documentation for a core library module.
fn get_core_module_doc(module: &str) -> String {
    match module {
        "Option" => {
            "Option handling utilities (is_some, is_none, unwrap_or, map, flatten)".to_string()
        }
        "Result" => {
            "Result handling utilities (is_ok, is_err, unwrap_or, map, map_err)".to_string()
        }
        "List" => "List operations (len, map, filter, fold)".to_string(),
        "string" => "String utilities (len, concat, from_number)".to_string(),
        "math" => "Mathematical functions (abs, min, max, clamp, PI, E, TAU)".to_string(),
        _ => format!("Core library module: {module}"),
    }
}

/// Get core submodule member completions (functions and constants).
fn get_core_submodule_member_completions(submodule: &str, prefix: &str) -> Vec<CompletionItem> {
    use std::sync::Arc;

    // Get the source code for the submodule
    let Ok(source) = CoreLibrary::get_source(&[Arc::from(submodule)]) else {
        return Vec::new();
    };

    // Parse the exports from the source
    let exports = parse_core_module_exports(source);

    // Filter and convert to completion items
    exports
        .into_iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, kind)| {
            let (item_kind, detail) = match kind {
                CoreExportKind::Function => (CompletionItemKind::FUNCTION, "function"),
                CoreExportKind::Const => (CompletionItemKind::CONSTANT, "constant"),
            };
            CompletionItem {
                label: name.clone(),
                kind: Some(item_kind),
                detail: Some(format!("core::{submodule}::{name} ({detail})")),
                ..Default::default()
            }
        })
        .collect()
}

/// Get completions for pkg module members from `SymbolDb`.
fn get_pkg_module_completions(
    db: &SymbolDb,
    module_path: &str,
    prefix: &str,
) -> Vec<CompletionItem> {
    // The symbol database keys modules by their internal dotted path.
    let Ok(symbols) = db.get_module_symbols(&module_path.replace("::", ".")) else {
        return Vec::new();
    };

    symbols
        .into_iter()
        .filter_map(|entry| {
            // Extract symbol name from path (last segment after the last dot)
            let name = entry.path.rsplit('.').next()?;
            if !name.starts_with(prefix) {
                return None;
            }

            let item_kind = match entry.kind {
                SymbolKind::Function => CompletionItemKind::FUNCTION,
                SymbolKind::Const => CompletionItemKind::CONSTANT,
                SymbolKind::Enum => CompletionItemKind::ENUM,
                SymbolKind::Ability => CompletionItemKind::INTERFACE,
            };

            let detail = format!("{module_path}::{name}");

            Some(CompletionItem {
                label: name.to_string(),
                kind: Some(item_kind),
                detail: Some(detail),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(module_path.to_string()),
                }),
                ..Default::default()
            })
        })
        .collect()
}

/// Kind of export from a core module.
#[derive(Debug, Clone, Copy)]
enum CoreExportKind {
    Function,
    Const,
}

/// Parse exports from core module source code.
/// This is a lightweight parser that extracts pub fn and const declarations.
fn parse_core_module_exports(source: &str) -> Vec<(String, CoreExportKind)> {
    let mut exports = Vec::new();

    for line in source.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.starts_with("//") || line.is_empty() {
            continue;
        }

        // Match pub fn declarations
        if let Some(rest) = line.strip_prefix("pub fn ") {
            if let Some(name) = extract_core_identifier(rest) {
                exports.push((name, CoreExportKind::Function));
            }
        }
        // Match const declarations
        else if let Some(rest) = line.strip_prefix("const ") {
            if let Some(name) = extract_core_identifier(rest) {
                exports.push((name, CoreExportKind::Const));
            }
        }
        // Match pub const declarations
        else if let Some(rest) = line.strip_prefix("pub const ") {
            if let Some(name) = extract_core_identifier(rest) {
                exports.push((name, CoreExportKind::Const));
            }
        }
    }

    exports
}

/// Extract an identifier from the start of a string.
fn extract_core_identifier(s: &str) -> Option<String> {
    let s = s.trim();
    // Handle generic parameters: "len<T>(" -> "len"
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if end > 0 {
        Some(s[..end].to_string())
    } else {
        None
    }
}

/// Get use statement prefix completions (pkg, core, self, super).
fn get_use_prefix_completions(prefix: &str) -> Vec<CompletionItem> {
    let prefixes = [
        ("pkg", "Package-relative import (from package root)"),
        ("core", "Core library import"),
        ("self", "Relative import (same directory)"),
        ("super", "Parent directory import"),
    ];

    prefixes
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, doc)| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("import prefix".to_string()),
            documentation: Some(lsp_types::Documentation::String((*doc).to_string())),
            ..Default::default()
        })
        .collect()
}

/// Ability method descriptor for completions.
struct AbilityMethod {
    /// Method name.
    name: &'static str,
    /// Type signature (e.g., "(message: string): ()").
    signature: &'static str,
    /// Documentation string.
    doc: &'static str,
    /// Parameter names for snippet placeholders.
    params: &'static [&'static str],
}

/// Get the methods for a given ability.
#[allow(clippy::too_many_lines)]
fn get_ability_methods(ability_name: &str) -> &'static [AbilityMethod] {
    match ability_name {
        "Console" => &[
            AbilityMethod {
                name: "print",
                signature: "(message: string): ()",
                doc: "Print a message to stdout",
                params: &["message"],
            },
            AbilityMethod {
                name: "eprint",
                signature: "(message: string): ()",
                doc: "Print a message to stderr",
                params: &["message"],
            },
            AbilityMethod {
                name: "println",
                signature: "(message: string): ()",
                doc: "Print a message with newline",
                params: &["message"],
            },
        ],
        "Exception" => &[AbilityMethod {
            name: "throw",
            signature: "(error: E): !",
            doc: "Throw an exception",
            params: &["error"],
        }],
        "Time" => &[
            AbilityMethod {
                name: "now",
                signature: "(): Timestamp",
                doc: "Get current timestamp",
                params: &[],
            },
            AbilityMethod {
                name: "wait",
                signature: "(duration: Duration): ()",
                doc: "Wait for a duration",
                params: &["duration"],
            },
        ],
        "Random" => &[
            AbilityMethod {
                name: "seed",
                signature: "(): number",
                doc: "Get a random number 0.0 to 1.0",
                params: &[],
            },
            AbilityMethod {
                name: "in_range",
                signature: "(range: Range): number",
                doc: "Get random number in range",
                params: &["range"],
            },
        ],
        "Log" => &[
            AbilityMethod {
                name: "debug",
                signature: "(message: string): ()",
                doc: "Log debug message",
                params: &["message"],
            },
            AbilityMethod {
                name: "info",
                signature: "(message: string): ()",
                doc: "Log info message",
                params: &["message"],
            },
            AbilityMethod {
                name: "warn",
                signature: "(message: string): ()",
                doc: "Log warning message",
                params: &["message"],
            },
            AbilityMethod {
                name: "error",
                signature: "(message: string): ()",
                doc: "Log error message",
                params: &["message"],
            },
        ],
        "FileSystem" => &[
            AbilityMethod {
                name: "read",
                signature: "(path: string): string",
                doc: "Read a file as UTF-8 text",
                params: &["path"],
            },
            AbilityMethod {
                name: "write",
                signature: "(path: string, content: string): ()",
                doc: "Write (create/truncate) a file with text",
                params: &["path", "content"],
            },
            AbilityMethod {
                name: "read_bytes",
                signature: "(path: string): Bytes",
                doc: "Read a file as raw bytes",
                params: &["path"],
            },
            AbilityMethod {
                name: "write_bytes",
                signature: "(path: string, data: Bytes): ()",
                doc: "Write (create/truncate) a file with bytes",
                params: &["path", "data"],
            },
            AbilityMethod {
                name: "exists",
                signature: "(path: string): bool",
                doc: "Check whether a path exists",
                params: &["path"],
            },
            AbilityMethod {
                name: "list",
                signature: "(path: string): List<string>",
                doc: "List directory entry names (sorted)",
                params: &["path"],
            },
            AbilityMethod {
                name: "remove",
                signature: "(path: string): ()",
                doc: "Remove a file or empty directory",
                params: &["path"],
            },
            AbilityMethod {
                name: "create_dir",
                signature: "(path: string): ()",
                doc: "Create a directory and any missing parents",
                params: &["path"],
            },
        ],
        "Network" => &[AbilityMethod {
            name: "fetch",
            signature: "(request: Request): Response",
            doc: "Fetch a URL",
            params: &["request"],
        }],
        _ => &[],
    }
}

/// Build a snippet string for an ability method call.
///
/// For methods with no parameters: `!()`
/// For methods with parameters: `!(${1:param1}, ${2:param2})`
fn build_method_snippet(method: &AbilityMethod) -> String {
    if method.params.is_empty() {
        format!("{}!()", method.name)
    } else {
        let placeholders: Vec<String> = method
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| format!("${{{}:{}}}", i + 1, p))
            .collect();
        format!("{}!({})", method.name, placeholders.join(", "))
    }
}

/// Get ability method completions.
fn get_ability_method_completions(ability_name: &str, prefix: &str) -> Vec<CompletionItem> {
    let methods = get_ability_methods(ability_name);

    methods
        .iter()
        .filter(|m| m.name.starts_with(prefix))
        .map(|m| {
            let snippet = build_method_snippet(m);
            CompletionItem {
                // Show the method name with ! in the completion list
                label: format!("{}!", m.name),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format!("{}!{}", m.name, m.signature)),
                label_details: Some(CompletionItemLabelDetails {
                    detail: Some(m.signature.to_string()),
                    description: None,
                }),
                documentation: Some(lsp_types::Documentation::String(m.doc.to_string())),
                // Use snippet format for parameter placeholders
                insert_text: Some(snippet),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            }
        })
        .collect()
}

/// Get function completions from the module.
fn get_function_completions(module: &Module, prefix: &str) -> Vec<CompletionItem> {
    module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Function(func) = &item.kind {
                let name = func.name.as_ref();
                if name.starts_with(prefix) {
                    Some(function_to_completion(func))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Convert a function definition to a completion item.
fn function_to_completion(func: &FunctionDef) -> CompletionItem {
    let name = func.name.to_string();

    // Build parameter signature.
    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| {
            if let Some(ty) = &p.ty {
                format!("{}: {}", p.name, format_type(ty))
            } else {
                p.name.to_string()
            }
        })
        .collect();

    let ret = func.ret_ty.as_ref().map_or("_".to_string(), format_type);

    let signature = format!("({}): {}", params.join(", "), ret);

    let detail = if func.is_public {
        format!("pub fn {name}{signature}")
    } else {
        format!("fn {name}{signature}")
    };

    CompletionItem {
        label: name,
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(detail),
        label_details: Some(CompletionItemLabelDetails {
            detail: Some(signature),
            description: None,
        }),
        ..Default::default()
    }
}

/// Get local variable completions.
fn get_local_completions(module: &Module, offset: usize, prefix: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    // Find the function containing the offset.
    for item in &module.items {
        if let ItemKind::Function(func) = &item.kind {
            // Check if offset is within this function's span.
            if offset >= item.span.start && offset <= item.span.end {
                // Add function parameters.
                for param in &func.params {
                    if param.name.starts_with(prefix) {
                        items.push(param_to_completion(param));
                    }
                }

                // Add local variables from the body.
                collect_locals_in_scope(&func.body, offset, prefix, &mut items);
            }
        }
    }

    items
}

/// Convert a parameter to a completion item.
fn param_to_completion(param: &Param) -> CompletionItem {
    let name = param.name.to_string();
    let type_str = param.ty.as_ref().map_or("_".to_string(), format_type);

    CompletionItem {
        label: name.clone(),
        kind: Some(CompletionItemKind::VARIABLE),
        detail: Some(format!("{name}: {type_str}")),
        label_details: Some(CompletionItemLabelDetails {
            detail: Some(format!(": {type_str}")),
            description: Some("parameter".to_string()),
        }),
        ..Default::default()
    }
}

/// Add parameters as completions if we're inside the given body span.
fn collect_params_if_in_scope(
    params: &[Param],
    body_span: ambient_engine::ast::Span,
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    if offset >= body_span.start && offset <= body_span.end {
        for param in params {
            if param.name.starts_with(prefix) {
                items.push(param_to_completion(param));
            }
        }
    }
}

/// Collect local variables that are in scope at the given offset.
fn collect_locals_in_scope(
    expr: &Expr,
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    // Only look inside if the expression contains the offset.
    if offset < expr.span.start || offset > expr.span.end {
        return;
    }

    match &expr.kind {
        ExprKind::Block(stmts, result) => {
            collect_block_locals(stmts, result.as_deref(), offset, prefix, items);
        }
        ExprKind::If(cond, then_branch, else_branch) => {
            collect_locals_in_scope(cond, offset, prefix, items);
            collect_locals_in_scope(then_branch, offset, prefix, items);
            if let Some(else_branch) = else_branch {
                collect_locals_in_scope(else_branch, offset, prefix, items);
            }
        }
        ExprKind::Match(scrutinee, arms) => {
            collect_match_locals(scrutinee, arms, offset, prefix, items);
        }
        ExprKind::Lambda(lambda) => {
            collect_lambda_locals(lambda, offset, prefix, items);
        }
        ExprKind::Handle(handle) => {
            collect_handle_locals(handle, offset, prefix, items);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_locals_in_scope(left, offset, prefix, items);
            collect_locals_in_scope(right, offset, prefix, items);
        }
        ExprKind::Unary(_, operand) => {
            collect_locals_in_scope(operand, offset, prefix, items);
        }
        ExprKind::Call(callee, args) => {
            collect_locals_in_scope(callee, offset, prefix, items);
            for arg in args {
                collect_locals_in_scope(arg, offset, prefix, items);
            }
        }
        ExprKind::Tuple(elements) | ExprKind::List(elements) => {
            for elem in elements {
                collect_locals_in_scope(elem, offset, prefix, items);
            }
        }
        ExprKind::Record(fields) | ExprKind::TypedRecord { fields, .. } => {
            for (_, value) in fields {
                collect_locals_in_scope(value, offset, prefix, items);
            }
        }
        ExprKind::RecordField(object, _) => collect_locals_in_scope(object, offset, prefix, items),
        ExprKind::TupleIndex(tuple, _) => collect_locals_in_scope(tuple, offset, prefix, items),
        ExprKind::Sandbox(sandbox) => {
            collect_locals_in_scope(&sandbox.body, offset, prefix, items);
        }
        // Leaf nodes - nothing to recurse into.
        ExprKind::Unit
        | ExprKind::Bool(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Local(_)
        | ExprKind::Name(_)
        | ExprKind::Perform(_)
        | ExprKind::Resume(_)
        | ExprKind::HandlerLiteral(_)
        | ExprKind::MethodCall { .. } => {}
    }
}

/// Collect locals from a block expression.
fn collect_block_locals(
    stmts: &[ambient_engine::ast::Stmt],
    result: Option<&Expr>,
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    for stmt in stmts {
        if let StmtKind::Let(binding) = &stmt.kind {
            // Only include if the binding is before the cursor.
            if stmt.span.end < offset && binding.name.starts_with(prefix) {
                let type_str = binding.ty.as_ref().map_or_else(
                    || {
                        binding
                            .init
                            .ty
                            .as_ref()
                            .map_or("_".to_string(), format_type)
                    },
                    format_type,
                );

                items.push(CompletionItem {
                    label: binding.name.to_string(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(format!("{}: {type_str}", binding.name)),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: Some(format!(": {type_str}")),
                        description: Some("local".to_string()),
                    }),
                    ..Default::default()
                });
            }
            collect_locals_in_scope(&binding.init, offset, prefix, items);
        }
    }

    if let Some(result) = result {
        collect_locals_in_scope(result, offset, prefix, items);
    }
}

/// Collect locals from a match expression.
fn collect_match_locals(
    scrutinee: &Expr,
    arms: &[ambient_engine::ast::MatchArm],
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    collect_locals_in_scope(scrutinee, offset, prefix, items);
    for arm in arms {
        // Add pattern bindings if we're inside this arm.
        if offset >= arm.body.span.start && offset <= arm.body.span.end {
            collect_pattern_bindings(&arm.pattern, prefix, items);
        }
        collect_locals_in_scope(&arm.body, offset, prefix, items);
    }
}

/// Collect locals from a lambda expression.
fn collect_lambda_locals(
    lambda: &ambient_engine::ast::Lambda,
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    collect_params_if_in_scope(&lambda.params, lambda.body.span, offset, prefix, items);
    collect_locals_in_scope(&lambda.body, offset, prefix, items);
}

/// Collect locals from a handle expression.
fn collect_handle_locals(
    handle: &ambient_engine::ast::HandleExpr,
    offset: u32,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    collect_locals_in_scope(&handle.body, offset, prefix, items);
    for handler in &handle.handlers {
        collect_params_if_in_scope(&handler.params, handler.body.span, offset, prefix, items);
        collect_locals_in_scope(&handler.body, offset, prefix, items);
    }
    if let Some(else_clause) = &handle.else_clause {
        collect_locals_in_scope(else_clause, offset, prefix, items);
    }
}

/// Collect bindings from a pattern.
fn collect_pattern_bindings(
    pattern: &ambient_engine::ast::Pattern,
    prefix: &str,
    items: &mut Vec<CompletionItem>,
) {
    use ambient_engine::ast::PatternKind;

    match &pattern.kind {
        PatternKind::Binding(_, name) => {
            if name.starts_with(prefix) {
                items.push(CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(format!("{name}: _")),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: None,
                        description: Some("pattern binding".to_string()),
                    }),
                    ..Default::default()
                });
            }
        }
        PatternKind::Tuple(elements) => {
            for elem in elements {
                collect_pattern_bindings(elem, prefix, items);
            }
        }
        PatternKind::Record(fields) => {
            for (_, pattern) in fields {
                collect_pattern_bindings(pattern, prefix, items);
            }
        }
        PatternKind::Variant(_, payload) => {
            if let Some(payload) = payload {
                collect_pattern_bindings(payload, prefix, items);
            }
        }
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_context_simple() {
        let source = "fn foo() { let x = 1; x }";
        let ctx = CompletionContext::new(source, 23); // cursor at 'x' near end

        assert_eq!(ctx.word_prefix, "x");
        assert!(!ctx.after_dot);
        assert!(ctx.after_ability_dot.is_none());
        assert!(!ctx.after_core_dot);
        assert!(!ctx.in_use_statement);
    }

    #[test]
    fn test_completion_context_after_scope() {
        let source = "Console::pr";
        let ctx = CompletionContext::new(source, 11);

        assert_eq!(ctx.word_prefix, "pr");
        assert!(!ctx.after_dot);
        assert_eq!(ctx.after_ability_dot, Some("Console"));
        assert!(!ctx.after_core_dot);
    }

    #[test]
    fn test_completion_context_after_core_scope() {
        let source = "use core::ma";
        let ctx = CompletionContext::new(source, 12);

        assert_eq!(ctx.word_prefix, "ma");
        assert!(!ctx.after_dot);
        assert!(ctx.after_ability_dot.is_none());
        assert!(ctx.after_core_dot);
        assert!(ctx.in_use_statement);
    }

    #[test]
    fn test_completion_context_in_use_statement() {
        let source = "use pk";
        let ctx = CompletionContext::new(source, 6);

        assert_eq!(ctx.word_prefix, "pk");
        assert!(!ctx.after_dot);
        assert!(ctx.in_use_statement);
    }

    #[test]
    fn test_keyword_completions() {
        let items = get_keyword_completions("le");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "let");
    }

    #[test]
    fn test_type_completions() {
        let items = get_type_completions("num");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "number");
    }

    #[test]
    fn test_ability_method_completions() {
        let items = get_ability_method_completions("Console", "pr");
        assert_eq!(items.len(), 2); // print and println
        assert!(items.iter().any(|i| i.label == "print!"));
        assert!(items.iter().any(|i| i.label == "println!"));

        // Check that insert_text includes the ! and snippet placeholders
        let print_item = items.iter().find(|i| i.label == "print!").unwrap();
        assert_eq!(
            print_item.insert_text.as_deref(),
            Some("print!(${1:message})")
        );
        assert_eq!(
            print_item.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );

        // Check zero-param methods
        let random_items = get_ability_method_completions("Random", "se");
        assert_eq!(random_items.len(), 1);
        let seed_item = &random_items[0];
        assert_eq!(seed_item.label, "seed!");
        assert_eq!(seed_item.insert_text.as_deref(), Some("seed!()"));
    }

    #[test]
    fn test_empty_prefix_returns_all_keywords() {
        let items = get_keyword_completions("");
        assert_eq!(items.len(), TokenKind::all_keywords().len());
    }

    #[test]
    fn test_core_module_completions() {
        let items = get_core_module_completions("ma");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "math");
        assert_eq!(items[0].kind, Some(CompletionItemKind::MODULE));
    }

    #[test]
    fn test_core_module_completions_all() {
        let items = get_core_module_completions("");
        assert!(items.len() >= 3); // List, string, math (+ traits)
        assert!(items.iter().any(|i| i.label == "List"));
        assert!(items.iter().any(|i| i.label == "string"));
        assert!(items.iter().any(|i| i.label == "math"));
    }

    #[test]
    fn test_use_prefix_completions() {
        let items = get_use_prefix_completions("c");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "core");

        let all_items = get_use_prefix_completions("");
        assert_eq!(all_items.len(), 4); // pkg, core, self, super
    }
}
