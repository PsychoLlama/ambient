//! Locate a local binding's declaration span by its [`BindingId`].
//!
//! A pure AST walk over a function body used by go-to-definition on a local
//! (`ExprKind::Local`): given the binding id a reference carries, find the
//! `let`/parameter/pattern that introduced it. Split from the module root to
//! keep it under the per-file line budget.

use ambient_engine::ast::{
    BindingId, Expr, ExprKind, FunctionDef, Pattern, PatternKind, Span, StmtKind,
};

/// The declaration span of the binding `target_id` within `func` (a parameter
/// or a `let`/pattern binding in its body), if it declares one.
pub(super) fn find_binding_definition(func: &FunctionDef, target_id: BindingId) -> Option<Span> {
    // Check parameters.
    for param in &func.params {
        if param.id == target_id {
            return Some(param.span);
        }
    }

    // Check body.
    find_binding_in_expr(&func.body, target_id)
}

fn find_binding_in_expr(expr: &Expr, target_id: BindingId) -> Option<Span> {
    match &expr.kind {
        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                if let StmtKind::Let(binding) = &stmt.kind {
                    if binding.id == target_id {
                        return Some(stmt.span);
                    }
                    if let Some(span) = find_binding_in_expr(&binding.init, target_id) {
                        return Some(span);
                    }
                }
                if let StmtKind::Const(const_def) = &stmt.kind {
                    if const_def.id == target_id {
                        return Some(stmt.span);
                    }
                    if let Some(span) = find_binding_in_expr(&const_def.value, target_id) {
                        return Some(span);
                    }
                }
            }
            if let Some(result) = result {
                find_binding_in_expr(result, target_id)
            } else {
                None
            }
        }
        ExprKind::Match(scrutinee, arms) => {
            if let Some(span) = find_binding_in_expr(scrutinee, target_id) {
                return Some(span);
            }
            for arm in arms {
                if let Some(span) = find_binding_in_pattern(&arm.pattern, target_id) {
                    return Some(span);
                }
                if let Some(span) = find_binding_in_expr(&arm.body, target_id) {
                    return Some(span);
                }
            }
            None
        }
        ExprKind::Lambda(lambda) => {
            for param in &lambda.params {
                if param.id == target_id {
                    return Some(param.span);
                }
            }
            find_binding_in_expr(&lambda.body, target_id)
        }
        ExprKind::If(condition, then_branch, else_branch) => {
            find_binding_in_expr(condition, target_id)
                .or_else(|| find_binding_in_expr(then_branch, target_id))
                .or_else(|| {
                    else_branch
                        .as_ref()
                        .and_then(|e| find_binding_in_expr(e, target_id))
                })
        }
        ExprKind::Binary { left, right, .. } => {
            find_binding_in_expr(left, target_id).or_else(|| find_binding_in_expr(right, target_id))
        }
        ExprKind::Unary(_, operand) => find_binding_in_expr(operand, target_id),
        ExprKind::Call(callee, args) => find_binding_in_expr(callee, target_id)
            .or_else(|| args.iter().find_map(|a| find_binding_in_expr(a, target_id))),
        ExprKind::Tuple(elements) | ExprKind::List(elements) => elements
            .iter()
            .find_map(|e| find_binding_in_expr(e, target_id)),
        ExprKind::Record(fields) => fields
            .iter()
            .find_map(|(_, e)| find_binding_in_expr(e, target_id)),
        ExprKind::RecordField(object, _) => find_binding_in_expr(object, target_id),
        ExprKind::TupleIndex(tuple, _) => find_binding_in_expr(tuple, target_id),
        ExprKind::Handle(handle) => find_binding_in_expr(&handle.body, target_id)
            .or_else(|| {
                handle
                    .handlers
                    .iter()
                    .find_map(|h| find_binding_in_expr(h, target_id))
            })
            .or_else(|| {
                handle
                    .else_clause
                    .as_ref()
                    .and_then(|e| find_binding_in_expr(e, target_id))
            }),
        ExprKind::Sandbox(sandbox) => find_binding_in_expr(&sandbox.body, target_id),
        _ => None,
    }
}

fn find_binding_in_pattern(pattern: &Pattern, target_id: BindingId) -> Option<Span> {
    match &pattern.kind {
        PatternKind::Binding(id, _) => {
            if *id == target_id {
                Some(pattern.span)
            } else {
                None
            }
        }
        PatternKind::Tuple(elements) => elements
            .iter()
            .find_map(|p| find_binding_in_pattern(p, target_id)),
        PatternKind::Record(fields) => fields
            .iter()
            .find_map(|(_, p)| find_binding_in_pattern(p, target_id)),
        PatternKind::Variant(_, payload) => payload
            .as_ref()
            .and_then(|p| find_binding_in_pattern(p, target_id)),
        _ => None,
    }
}
