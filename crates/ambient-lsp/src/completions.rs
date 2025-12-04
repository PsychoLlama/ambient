//! Code completion support for the Ambient language.
//!
//! Provides auto-completion for:
//! - Keywords (fn, let, if, etc.)
//! - Built-in types (number, string, bool)
//! - Function names from the current module
//! - Local variables in scope
//! - Ability names and methods

use ambient_engine::ast::{Expr, ExprKind, FunctionDef, ItemKind, Module, Param, StmtKind};
use ambient_parser::TokenKind;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails};

use crate::analysis::format_type;

/// A completion context containing information about the cursor position.
#[derive(Debug)]
pub struct CompletionContext<'a> {
    /// The byte offset of the cursor.
    pub offset: usize,
    /// The word being typed (prefix for filtering).
    pub word_prefix: &'a str,
    /// Whether we're after a dot (field/method access).
    pub after_dot: bool,
    /// Whether we're after an ability name (for method completion).
    pub after_ability_dot: Option<&'a str>,
}

impl<'a> CompletionContext<'a> {
    /// Create a completion context from source and offset.
    #[must_use]
    pub fn new(source: &'a str, offset: usize) -> Self {
        let offset = offset.min(source.len());

        // Find the start of the current line.
        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_prefix = &source[line_start..offset];

        // Find the word being typed.
        let word_start = line_prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map_or(0, |i| i + 1);
        let word_prefix = &line_prefix[word_start..];

        // Check if we're after a dot.
        let before_word = &line_prefix[..word_start];
        let after_dot = before_word.trim_end().ends_with('.');

        // Check if we're after an ability name (e.g., "Console.")
        let after_ability_dot = if after_dot {
            // Look for the identifier before the dot
            let trimmed = before_word.trim_end();
            let without_dot = trimmed.strip_suffix('.').unwrap_or(trimmed);
            let ident_start = without_dot
                .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                .map_or(0, |i| i + 1);
            let ident = &without_dot[ident_start..];
            if TokenKind::builtin_abilities().contains(&ident) {
                Some(ident)
            } else {
                None
            }
        } else {
            None
        };

        Self {
            offset,
            word_prefix,
            after_dot,
            after_ability_dot,
        }
    }
}

/// Generate completions for a given context.
#[must_use]
pub fn get_completions(
    ctx: &CompletionContext<'_>,
    module: Option<&Module>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // If we're completing an ability method, show ability methods.
    if let Some(ability_name) = ctx.after_ability_dot {
        items.extend(get_ability_method_completions(ability_name, ctx.word_prefix));
        return items;
    }

    // If we're after a dot (but not an ability), we'd show field completions.
    // For now, we don't have enough type info at the cursor, so skip.
    if ctx.after_dot {
        return items;
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

/// Get ability method completions.
fn get_ability_method_completions(ability_name: &str, prefix: &str) -> Vec<CompletionItem> {
    let methods: &[(&str, &str, &str)] = match ability_name {
        "Console" => &[
            ("print", "(message: string): ()", "Print a message to stdout"),
            ("eprint", "(message: string): ()", "Print a message to stderr"),
            (
                "println",
                "(message: string): ()",
                "Print a message with newline",
            ),
        ],
        "Exception" => &[("throw", "(error: E): !", "Throw an exception")],
        "Time" => &[
            ("now", "(): Timestamp", "Get current timestamp"),
            ("wait", "(duration: Duration): ()", "Wait for a duration"),
        ],
        "Random" => &[
            ("seed", "(): number", "Get a random number 0.0 to 1.0"),
            (
                "in_range",
                "(range: Range): number",
                "Get random number in range",
            ),
        ],
        "Async" => &[
            (
                "all",
                "<T, A!>(ops: List<Ability<T, A!>>): List<T>",
                "Wait for all operations",
            ),
            (
                "race",
                "<T, A!>(ops: List<Ability<T, A!>>): T",
                "Race operations, first wins",
            ),
        ],
        "Log" => &[
            ("debug", "(message: string): ()", "Log debug message"),
            ("info", "(message: string): ()", "Log info message"),
            ("warn", "(message: string): ()", "Log warning message"),
            ("error", "(message: string): ()", "Log error message"),
        ],
        "Filesystem" => &[
            ("read", "(path: Path): string", "Read file contents"),
            (
                "write",
                "(path: Path, content: string): ()",
                "Write file contents",
            ),
            ("exists", "(path: Path): bool", "Check if file exists"),
        ],
        "Network" => &[("fetch", "(request: Request): Response", "Fetch a URL")],
        _ => &[],
    };

    methods
        .iter()
        .filter(|(name, _, _)| name.starts_with(prefix))
        .map(|(name, sig, doc)| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("{name}{sig}")),
            label_details: Some(CompletionItemLabelDetails {
                detail: Some((*sig).to_string()),
                description: None,
            }),
            documentation: Some(lsp_types::Documentation::String((*doc).to_string())),
            ..Default::default()
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

    let ret = func
        .ret_ty
        .as_ref()
        .map_or("_".to_string(), format_type);

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
        ExprKind::Binary(_, left, right) => {
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
        ExprKind::Record(fields) => {
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
        | ExprKind::Suspend(_)
        | ExprKind::Resume(_)
        | ExprKind::HandlerLiteral(_) => {}
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
                    || binding.init.ty.as_ref().map_or("_".to_string(), format_type),
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
    // Add lambda parameters if we're inside the lambda body.
    if offset >= lambda.body.span.start && offset <= lambda.body.span.end {
        for param in &lambda.params {
            if param.name.starts_with(prefix) {
                items.push(param_to_completion(param));
            }
        }
    }
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
        // Add handler parameters if we're inside the handler body.
        if offset >= handler.body.span.start && offset <= handler.body.span.end {
            for param in &handler.params {
                if param.name.starts_with(prefix) {
                    items.push(param_to_completion(param));
                }
            }
        }
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
    }

    #[test]
    fn test_completion_context_after_dot() {
        let source = "Console.pr";
        let ctx = CompletionContext::new(source, 10);

        assert_eq!(ctx.word_prefix, "pr");
        assert!(ctx.after_dot);
        assert_eq!(ctx.after_ability_dot, Some("Console"));
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
        assert!(items.iter().any(|i| i.label == "print"));
        assert!(items.iter().any(|i| i.label == "println"));
    }

    #[test]
    fn test_empty_prefix_returns_all_keywords() {
        let items = get_keyword_completions("");
        assert_eq!(items.len(), TokenKind::all_keywords().len());
    }
}
