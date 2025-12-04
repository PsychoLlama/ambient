//! Code analysis for the Ambient language.
//!
//! This module handles parsing and type checking, producing diagnostics
//! and type information for the LSP server.

use ambient_engine::ast::{Expr, ExprKind, ItemKind, Module, Span, StmtKind};
use ambient_engine::infer::{check_module, TypeError};
use ambient_engine::types::Type;
use ambient_parser::{parse, ParseError};

/// The result of analyzing a document.
#[derive(Debug)]
pub struct AnalysisResult {
    /// Parse error, if any.
    pub parse_error: Option<ParseError>,
    /// Type errors (empty if parsing failed).
    pub type_errors: Vec<TypeError>,
    /// The typed AST module (None if parsing failed).
    pub module: Option<Module>,
}

impl AnalysisResult {
    /// Check if the analysis found any errors.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.parse_error.is_some() || !self.type_errors.is_empty()
    }
}

/// Analyze a document's source code.
///
/// Performs parsing and type checking, returning all diagnostics found.
#[must_use]
pub fn analyze(source: &str) -> AnalysisResult {
    // Parse the source.
    let module = match parse(source) {
        Ok(m) => m,
        Err(e) => {
            return AnalysisResult {
                parse_error: Some(e),
                type_errors: Vec::new(),
                module: None,
            };
        }
    };

    // Type check the module.
    let check_result = check_module(module);

    AnalysisResult {
        parse_error: None,
        type_errors: check_result.errors,
        module: Some(check_result.module),
    }
}

/// Find the expression at a given byte offset in the module.
#[must_use]
pub fn find_expr_at_offset(module: &Module, offset: u32) -> Option<&Expr> {
    for item in &module.items {
        if let Some(expr) = find_expr_in_item_at_offset(&item.kind, offset) {
            return Some(expr);
        }
    }
    None
}

fn find_expr_in_item_at_offset(item: &ItemKind, offset: u32) -> Option<&Expr> {
    match item {
        ItemKind::Function(f) => find_expr_in_tree(&f.body, offset),
        ItemKind::Const(c) => find_expr_in_tree(&c.value, offset),
        _ => None,
    }
}

fn find_expr_in_tree(expr: &Expr, offset: u32) -> Option<&Expr> {
    // Check if offset is within this expression's span.
    if offset < expr.span.start || offset > expr.span.end {
        return None;
    }

    // Try to find a more specific child expression.
    let child = match &expr.kind {
        ExprKind::Binary(_, left, right) => {
            find_expr_in_tree(left, offset).or_else(|| find_expr_in_tree(right, offset))
        }
        ExprKind::Unary(_, operand) => find_expr_in_tree(operand, offset),
        ExprKind::Call(callee, args) => find_expr_in_tree(callee, offset)
            .or_else(|| args.iter().find_map(|a| find_expr_in_tree(a, offset))),
        ExprKind::If(condition, then_branch, else_branch) => find_expr_in_tree(condition, offset)
            .or_else(|| find_expr_in_tree(then_branch, offset))
            .or_else(|| {
                else_branch
                    .as_ref()
                    .and_then(|e| find_expr_in_tree(e, offset))
            }),
        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                if let StmtKind::Let(binding) = &stmt.kind {
                    if let Some(e) = find_expr_in_tree(&binding.init, offset) {
                        return Some(e);
                    }
                }
            }
            result.as_ref().and_then(|e| find_expr_in_tree(e, offset))
        }
        ExprKind::Tuple(elements) | ExprKind::List(elements) => {
            elements.iter().find_map(|e| find_expr_in_tree(e, offset))
        }
        ExprKind::Record(fields) => fields
            .iter()
            .find_map(|(_, e)| find_expr_in_tree(e, offset)),
        ExprKind::RecordField(object, _) => find_expr_in_tree(object, offset),
        ExprKind::TupleIndex(tuple, _) => find_expr_in_tree(tuple, offset),
        ExprKind::Match(scrutinee, arms) => find_expr_in_tree(scrutinee, offset).or_else(|| {
            arms.iter()
                .find_map(|arm| find_expr_in_tree(&arm.body, offset))
        }),
        ExprKind::Lambda(lambda) => find_expr_in_tree(&lambda.body, offset),
        ExprKind::Handle(handle) => find_expr_in_tree(&handle.body, offset).or_else(|| {
            handle
                .else_clause
                .as_ref()
                .and_then(|e| find_expr_in_tree(e, offset))
        }),
        ExprKind::Sandbox(sandbox) => find_expr_in_tree(&sandbox.body, offset),
        // Leaf nodes
        ExprKind::Unit
        | ExprKind::Bool(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Local(_)
        | ExprKind::Name(_)
        | ExprKind::Perform(_)
        | ExprKind::Suspend(_)
        | ExprKind::Resume(_)
        | ExprKind::HandlerLiteral(_) => None,
    };

    // Return the child if found, otherwise this expression.
    child.or(Some(expr))
}

/// Format a type for display in hover information.
#[must_use]
pub fn format_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "()".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Number => "number".to_string(),
        Type::String => "string".to_string(),
        Type::Var(var) => match var {
            ambient_engine::types::TypeVar::Unbound(id) => format!("?{id}"),
            ambient_engine::types::TypeVar::Link(linked) => format_type(&linked.borrow()),
        },
        Type::Function(ft) => {
            let params: Vec<_> = ft.params.iter().map(format_type).collect();
            let ret = format_type(&ft.ret);
            let abilities = if ft.abilities.is_empty() {
                String::new()
            } else {
                format!(" with {}", format_ability_set(&ft.abilities))
            };
            format!("({}) -> {}{}", params.join(", "), ret, abilities)
        }
        Type::Tuple(elements) => {
            let parts: Vec<_> = elements.iter().map(format_type).collect();
            format!("({})", parts.join(", "))
        }
        Type::Record(rec) => {
            let fields: Vec<_> = rec
                .fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", name, format_type(ty)))
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        Type::Named(named) => {
            if named.args.is_empty() {
                named.name.to_string()
            } else {
                let args: Vec<_> = named.args.iter().map(format_type).collect();
                format!("{}<{}>", named.name, args.join(", "))
            }
        }
        Type::Nominal(nom) => nom
            .name
            .as_ref()
            .map_or_else(|| "nominal".to_string(), ToString::to_string),
        Type::Handler(handler) => format!("Handler<#{}>", handler.ability),
        Type::Forall(forall) => {
            let vars: Vec<_> = forall.vars.iter().map(|v| format!("'{v}")).collect();
            format!("forall {}. {}", vars.join(" "), format_type(&forall.body))
        }
        Type::AbilityValue(av) => {
            format!(
                "Ability<{}, {}!>",
                format_type(&av.result),
                format_ability_set(&av.ability)
            )
        }
        Type::Never => "!".to_string(),
        Type::Error => "<error>".to_string(),
        Type::Hole => "_".to_string(),
    }
}

fn format_ability_set(abilities: &ambient_engine::types::AbilitySet) -> String {
    use ambient_engine::types::AbilitySet;

    match abilities {
        AbilitySet::Empty => "pure".to_string(),
        AbilitySet::Concrete(ids) => {
            let parts: Vec<_> = ids.iter().map(|a| format!("#{a}")).collect();
            parts.join(", ")
        }
        AbilitySet::Var(var_id) => format!("E{var_id}!"),
        AbilitySet::Row { concrete, tail } => {
            let mut parts: Vec<_> = concrete.iter().map(|a| format!("#{a}")).collect();
            parts.push(format!("E{tail}!"));
            parts.join(", ")
        }
    }
}

/// Find definition location for a variable at the given offset.
#[must_use]
pub fn find_definition(module: &Module, offset: u32) -> Option<DefinitionResult> {
    // First, find what's at the offset.
    let expr = find_expr_at_offset(module, offset)?;

    match &expr.kind {
        ExprKind::Local(binding_id) => {
            // Find the binding in the module.
            for item in &module.items {
                if let ItemKind::Function(func) = &item.kind {
                    if let Some(def_span) = find_binding_definition(func, *binding_id) {
                        return Some(DefinitionResult { span: def_span });
                    }
                }
            }
            None
        }
        ExprKind::Name(qname) => {
            // Check if it's a top-level function name.
            for item in &module.items {
                if let ItemKind::Function(func) = &item.kind {
                    if func.name.as_ref() == qname.name.as_ref() {
                        return Some(DefinitionResult { span: item.span });
                    }
                }
            }
            None
        }
        ExprKind::Call(callee, _) => {
            // Recurse into the callee.
            find_definition_in_expr(module, callee)
        }
        _ => None,
    }
}

fn find_definition_in_expr(module: &Module, expr: &Expr) -> Option<DefinitionResult> {
    match &expr.kind {
        ExprKind::Name(qname) => {
            // Check if it's a top-level function name.
            for item in &module.items {
                if let ItemKind::Function(func) = &item.kind {
                    if func.name.as_ref() == qname.name.as_ref() {
                        return Some(DefinitionResult { span: item.span });
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_binding_definition(
    func: &ambient_engine::ast::FunctionDef,
    target_id: ambient_engine::ast::BindingId,
) -> Option<Span> {
    // Check parameters.
    for param in &func.params {
        if param.id == target_id {
            return Some(param.span);
        }
    }

    // Check body.
    find_binding_in_expr(&func.body, target_id)
}

fn find_binding_in_expr(expr: &Expr, target_id: ambient_engine::ast::BindingId) -> Option<Span> {
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
        ExprKind::Binary(_, left, right) => {
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
        ExprKind::Handle(handle) => find_binding_in_expr(&handle.body, target_id).or_else(|| {
            handle
                .else_clause
                .as_ref()
                .and_then(|e| find_binding_in_expr(e, target_id))
        }),
        ExprKind::Sandbox(sandbox) => find_binding_in_expr(&sandbox.body, target_id),
        _ => None,
    }
}

fn find_binding_in_pattern(
    pattern: &ambient_engine::ast::Pattern,
    target_id: ambient_engine::ast::BindingId,
) -> Option<Span> {
    use ambient_engine::ast::PatternKind;
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

/// The result of a definition lookup.
#[derive(Debug, Clone)]
pub struct DefinitionResult {
    /// The source span of the definition.
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_valid() {
        let source = "fn add(x: number, y: number): number { x + y }";
        let result = analyze(source);
        assert!(!result.has_errors());
        assert!(result.module.is_some());
    }

    #[test]
    fn test_analyze_parse_error() {
        let source = "fn broken(";
        let result = analyze(source);
        assert!(result.has_errors());
        assert!(result.parse_error.is_some());
    }

    #[test]
    fn test_analyze_type_error() {
        let source = "fn bad(): string { 42 }";
        let result = analyze(source);
        assert!(result.has_errors());
        assert!(!result.type_errors.is_empty());
    }

    #[test]
    fn test_format_type() {
        assert_eq!(format_type(&Type::Number), "number");
        assert_eq!(format_type(&Type::String), "string");
        assert_eq!(format_type(&Type::Bool), "bool");
        assert_eq!(format_type(&Type::Unit), "()");
    }
}
