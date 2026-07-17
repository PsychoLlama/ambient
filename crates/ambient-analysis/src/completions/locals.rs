//! Scope-aware completion of locals: parameters, `let` bindings, pattern
//! bindings, and `self` — collected from whatever body (free function or
//! impl method) contains the cursor.

use ambient_engine::ast::{Expr, ExprKind, ItemKind, Module, Param, StmtKind};

use super::{CompletionItem, CompletionKind};
use crate::queries::format_type;

/// Get local variable completions.
pub(super) fn get_local_completions(
    module: &Module,
    offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    // Find the body containing the offset: a free function's, or an impl
    // method's (whose scope also carries `self` and the method parameters).
    for item in &module.items {
        if offset < item.span.start || offset > item.span.end {
            continue;
        }
        match &item.kind {
            ItemKind::Function(func) => {
                for param in &func.params {
                    if param.name.starts_with(prefix) {
                        items.push(param_to_completion(param));
                    }
                }
                collect_locals_in_scope(&func.body, offset, prefix, &mut items);
            }
            ItemKind::Impl(impl_def) => {
                for method in &impl_def.methods {
                    if offset < method.span.start || offset > method.span.end {
                        continue;
                    }
                    if method.has_self && "self".starts_with(prefix) {
                        items.push(self_to_completion(&impl_def.for_type));
                    }
                    for param in &method.params {
                        if param.name.starts_with(prefix) {
                            items.push(param_to_completion(param));
                        }
                    }
                    collect_locals_in_scope(&method.body, offset, prefix, &mut items);
                }
            }
            _ => {}
        }
    }

    items
}

/// The `self` receiver as a completion item, typed as the impl's target.
fn self_to_completion(for_type: &ambient_engine::types::Type) -> CompletionItem {
    let type_str = format_type(for_type);
    CompletionItem {
        label: "self".to_string(),
        kind: CompletionKind::Variable,
        detail: Some(format!("self: {type_str}")),
        signature: Some(format!(": {type_str}")),
        description: Some("receiver".to_string()),
        ..Default::default()
    }
}

/// Convert a parameter to a completion item.
fn param_to_completion(param: &Param) -> CompletionItem {
    let name = param.name.to_string();
    let type_str = param.ty.as_ref().map_or("_".to_string(), format_type);

    CompletionItem {
        label: name.clone(),
        kind: CompletionKind::Variable,
        detail: Some(format!("{name}: {type_str}")),
        signature: Some(format!(": {type_str}")),
        description: Some("parameter".to_string()),
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
        ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_locals_in_scope(value, offset, prefix, items);
            }
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
                    kind: CompletionKind::Variable,
                    detail: Some(format!("{}: {type_str}", binding.name)),
                    signature: Some(format!(": {type_str}")),
                    description: Some("local".to_string()),
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
        // A handler literal's arm params are in scope within their bodies;
        // other handler expressions just contribute their own locals.
        if let ambient_engine::ast::ExprKind::HandlerLiteral(lit) = &handler.kind {
            for method in &lit.methods {
                collect_params_if_in_scope(&method.params, method.body.span, offset, prefix, items);
                collect_locals_in_scope(&method.body, offset, prefix, items);
            }
        } else {
            collect_locals_in_scope(handler, offset, prefix, items);
        }
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
                    kind: CompletionKind::Variable,
                    detail: Some(format!("{name}: _")),
                    description: Some("pattern binding".to_string()),
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
