//! Expression, statement, and pattern lowering, including
//! interpolated-string desugaring.

use std::sync::Arc;

use ambient_engine::ast::{
    AbilityCall, BinaryOp, Expr, ExprKind, HandleExpr, HandlerLiteralExpr, HandlerLiteralMethod,
    Lambda, LetBinding, Literal, MatchArm, Pattern, PatternKind, QualifiedName, SandboxExpr, Span,
    Stmt, StmtKind, UnaryOp,
};

use super::LoweringContext;
use super::items::{lower_const, lower_param, lower_use};
use super::types::{lower_qualified_name, lower_type};
use crate::cst::{
    CstBinaryOp, CstExpr, CstExprKind, CstLambda, CstLiteral, CstMatchArm, CstPattern,
    CstPatternKind, CstRecordPatternField, CstStmt, CstStmtKind, CstUnaryOp, StringPart,
};
use crate::error::{ParseError, ParseErrorKind};

#[allow(clippy::too_many_lines)]
pub(super) fn lower_expression(
    ctx: &mut LoweringContext,
    expr: &CstExpr,
) -> Result<Expr, ParseError> {
    let kind = match &expr.kind {
        CstExprKind::Unit => ExprKind::Unit,
        CstExprKind::Bool(b) => ExprKind::Bool(*b),
        CstExprKind::Number(n) => ExprKind::Number(*n),
        CstExprKind::String(s) => ExprKind::String(s.clone()),

        CstExprKind::InterpolatedString(parts) => {
            // Convert interpolated string to string concatenation
            // For now, we'll create a simplified representation
            lower_interpolated_string(ctx, parts, expr.span)?
        }

        CstExprKind::Ident(ident) => ExprKind::Name(QualifiedName::simple(ident.name.clone())),

        CstExprKind::QualifiedName(qn) => ExprKind::Name(lower_qualified_name(qn)),

        CstExprKind::Tuple(elements) => {
            let lowered = elements
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::Tuple(lowered)
        }

        CstExprKind::TupleIndex { tuple, index } => {
            let lowered = lower_expression(ctx, tuple)?;
            ExprKind::TupleIndex(Box::new(lowered), *index)
        }

        CstExprKind::Record(fields) => {
            let lowered = fields
                .iter()
                .map(|(name, value)| {
                    let lowered_value = lower_expression(ctx, value)?;
                    Ok((name.name.clone(), lowered_value))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;
            ExprKind::Record(lowered)
        }

        CstExprKind::TypedRecord { type_name, fields } => {
            let lowered_fields = fields
                .iter()
                .map(|(name, value)| {
                    let lowered_value = lower_expression(ctx, value)?;
                    Ok((name.name.clone(), lowered_value))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;
            ExprKind::TypedRecord {
                type_name: lower_qualified_name(type_name),
                fields: lowered_fields,
            }
        }

        CstExprKind::Field { record, field } => {
            let lowered = lower_expression(ctx, record)?;
            ExprKind::RecordField(Box::new(lowered), field.name.clone())
        }

        CstExprKind::List(elements) => {
            let lowered = elements
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::List(lowered)
        }

        CstExprKind::Binary { op, left, right } => {
            let ast_op = lower_binary_op(*op);
            let left = lower_expression(ctx, left)?;
            let right = lower_expression(ctx, right)?;
            ExprKind::Binary {
                op: ast_op,
                left: Box::new(left),
                right: Box::new(right),
                resolved_op: None,
            }
        }

        CstExprKind::Unary { op, operand } => {
            let ast_op = lower_unary_op(*op);
            let operand = lower_expression(ctx, operand)?;
            ExprKind::Unary(ast_op, Box::new(operand))
        }

        CstExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = lower_expression(ctx, condition)?;
            let then_br = lower_expression(ctx, then_branch)?;
            let else_br = else_branch
                .as_ref()
                .map(|e| lower_expression(ctx, e))
                .transpose()?;
            ExprKind::If(Box::new(cond), Box::new(then_br), else_br.map(Box::new))
        }

        CstExprKind::Match { scrutinee, arms } => {
            let scrut = lower_expression(ctx, scrutinee)?;
            let lowered_arms = arms
                .iter()
                .map(|arm| lower_match_arm(ctx, arm))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::Match(Box::new(scrut), lowered_arms)
        }

        CstExprKind::Block { stmts, result } => {
            let mut lowered_stmts = Vec::new();
            for s in stmts {
                lowered_stmts.extend(lower_stmt(ctx, s)?);
            }
            let lowered_result = result
                .as_ref()
                .map(|e| lower_expression(ctx, e))
                .transpose()?;
            ExprKind::Block(lowered_stmts, lowered_result.map(Box::new))
        }

        CstExprKind::Lambda(lambda) => {
            let lowered = lower_lambda(ctx, lambda)?;
            ExprKind::Lambda(lowered)
        }

        CstExprKind::Call { callee, args } => {
            let lowered_callee = lower_expression(ctx, callee)?;
            let lowered_args = args
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::Call(Box::new(lowered_callee), lowered_args)
        }

        CstExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            let lowered_receiver = lower_expression(ctx, receiver)?;
            let lowered_args = args
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::MethodCall {
                receiver: Box::new(lowered_receiver),
                method: method.name.clone(),
                method_span: Span::new(method.span.start, method.span.end),
                args: lowered_args,
                resolved_method: None,
            }
        }

        CstExprKind::Perform {
            ability,
            method,
            args,
        } => {
            let lowered_args = args
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::Perform(AbilityCall {
                ability: lower_qualified_name(ability),
                method: method.name.clone(),
                args: lowered_args,
                fingerprints: None,
                span: expr.span,
            })
        }

        CstExprKind::Handle(handle) => {
            // Lower the flat handler list (literals and value expressions
            // alike are ordinary expressions now).
            let handlers = handle
                .handlers
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;

            let body = lower_expression(ctx, &handle.body)?;

            let else_clause = handle
                .else_clause
                .as_ref()
                .map(|e| lower_expression(ctx, e))
                .transpose()?;

            ExprKind::Handle(HandleExpr {
                handlers,
                body: Box::new(body),
                else_clause: else_clause.map(Box::new),
            })
        }

        CstExprKind::Resume(value) => {
            let lowered_value = lower_expression(ctx, value)?;
            ExprKind::Resume(Box::new(lowered_value))
        }

        CstExprKind::HandlerLiteral(handler_lit) => {
            let methods = handler_lit
                .methods
                .iter()
                .map(|m| {
                    let params = m
                        .params
                        .iter()
                        .map(|p| lower_param(ctx, p))
                        .collect::<Result<Vec<_>, _>>()?;
                    let body = lower_expression(ctx, &m.body)?;

                    Ok(HandlerLiteralMethod {
                        ability: lower_qualified_name(&m.ability),
                        method: m.method.name.clone(),
                        method_span: m.method.span,
                        params,
                        body,
                        span: m.span,
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            ExprKind::HandlerLiteral(HandlerLiteralExpr {
                methods,
                span: handler_lit.span,
            })
        }

        CstExprKind::Sandbox(sandbox) => {
            let allowed_abilities = sandbox
                .allowed_abilities
                .iter()
                .map(lower_qualified_name)
                .collect();
            let body = lower_expression(ctx, &sandbox.body)?;

            ExprKind::Sandbox(SandboxExpr {
                allowed_abilities,
                body: Box::new(body),
                span: sandbox.span,
            })
        }

        CstExprKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error expression".into()),
                expr.span,
            ));
        }
    };

    Ok(Expr::new(kind, expr.span))
}

fn lower_interpolated_string(
    ctx: &mut LoweringContext,
    parts: &[StringPart],
    _span: Span,
) -> Result<ExprKind, ParseError> {
    // Desugar interpolated strings to nested string_concat calls.
    // For example: "hello ${name}!" becomes:
    //   string_concat(string_concat("hello ", to_string(name)), "!")
    //
    // Each expression part is wrapped in to_string() to ensure it becomes a string.

    // Handle empty case (shouldn't happen, but be safe)
    if parts.is_empty() {
        return Ok(ExprKind::String(Arc::from("")));
    }

    // Handle single literal case - no concatenation needed
    if parts.len() == 1
        && let StringPart::Literal(s, _) = &parts[0]
    {
        return Ok(ExprKind::String(s.clone()));
    }

    // Convert each part to an expression
    let mut exprs: Vec<Expr> = Vec::with_capacity(parts.len());
    for part in parts {
        let expr = match part {
            StringPart::Literal(s, part_span) => {
                // String literals are already strings
                Expr::new(ExprKind::String(s.clone()), *part_span)
            }
            StringPart::Expr(cst_expr) => {
                // Wrap expressions in to_string() call
                let inner = lower_expression(ctx, cst_expr)?;
                let inner_span = inner.span;
                make_to_string_call(inner, inner_span)
            }
        };
        exprs.push(expr);
    }

    // Filter out empty string literals (optimization)
    let exprs: Vec<Expr> = exprs
        .into_iter()
        .filter(|e| {
            if let ExprKind::String(s) = &e.kind {
                !s.is_empty()
            } else {
                true
            }
        })
        .collect();

    // Handle case where filtering left us with nothing
    if exprs.is_empty() {
        return Ok(ExprKind::String(Arc::from("")));
    }

    // Handle case where filtering left us with one item
    if exprs.len() == 1 {
        // SAFETY: We just checked len == 1, so next() will succeed
        return Ok(exprs
            .into_iter()
            .next()
            .unwrap_or_else(|| unreachable!())
            .kind);
    }

    // Chain all parts together with string_concat
    // We fold left-to-right: concat(concat(concat(a, b), c), d)
    let mut iter = exprs.into_iter();
    // SAFETY: We checked len >= 2, so next() will succeed
    let first = iter.next().unwrap_or_else(|| unreachable!());
    let result = iter.fold(first, |acc, next| {
        let result_span = Span::new(acc.span.start, next.span.end);
        make_string_concat_call(acc, next, result_span)
    });

    Ok(result.kind)
}

/// Create a `core::convert::to_string(expr)` call expression.
///
/// The canonical intrinsic path resolves in both the checker's and the
/// compiler's intrinsic tables; a bare `to_string` name would resolve in
/// neither.
fn make_to_string_call(expr: Expr, span: Span) -> Expr {
    let callee = Expr::new(
        ExprKind::Name(QualifiedName::qualified(
            vec![Arc::from("core"), Arc::from("convert")],
            Arc::from("to_string"),
        )),
        span,
    );
    Expr::new(ExprKind::Call(Box::new(callee), vec![expr]), span)
}

/// Create a `left.concat(right)` method call expression.
///
/// String's low-level externs are module-private; `concat` is reachable only
/// as the inherent method on `String`, so the desugar dispatches through it
/// exactly like user code writing `a.concat(b)`.
fn make_string_concat_call(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::new(
        ExprKind::MethodCall {
            receiver: Box::new(left),
            method: Arc::from("concat"),
            method_span: span,
            args: vec![right],
            resolved_method: None,
        },
        span,
    )
}

fn lower_stmt(ctx: &mut LoweringContext, stmt: &CstStmt) -> Result<Vec<Stmt>, ParseError> {
    let kind = match &stmt.kind {
        CstStmtKind::Let(binding) => {
            let id = ctx.fresh_binding();
            let ty = binding.ty.as_ref().map(lower_type).transpose()?;
            let init = lower_expression(ctx, &binding.init)?;

            StmtKind::Let(LetBinding {
                id,
                name: binding.name.name.clone(),
                name_span: binding.name.span,
                ty,
                init,
            })
        }
        CstStmtKind::Use(use_def) => {
            // A block-scoped use tree flattens exactly like a module-level
            // one: one statement per imported leaf.
            return Ok(lower_use(use_def)?
                .into_iter()
                .map(|flat| Stmt::new(StmtKind::Use(flat), stmt.span))
                .collect());
        }
        CstStmtKind::Const(const_def) => {
            // A block `const` lowers through the same path as a module-level
            // one, so it is content-addressed identically.
            StmtKind::Const(lower_const(ctx, const_def)?)
        }
        CstStmtKind::Expr(expr) => {
            let lowered = lower_expression(ctx, expr)?;
            StmtKind::Expr(lowered)
        }
        CstStmtKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error statement".into()),
                stmt.span,
            ));
        }
    };

    Ok(vec![Stmt::new(kind, stmt.span)])
}

fn lower_lambda(ctx: &mut LoweringContext, lambda: &CstLambda) -> Result<Lambda, ParseError> {
    let params = lambda
        .params
        .iter()
        .map(|p| lower_param(ctx, p))
        .collect::<Result<Vec<_>, _>>()?;

    let body = lower_expression(ctx, &lambda.body)?;

    Ok(Lambda {
        params,
        body: Box::new(body),
    })
}

fn lower_match_arm(ctx: &mut LoweringContext, arm: &CstMatchArm) -> Result<MatchArm, ParseError> {
    let pattern = lower_pattern(ctx, &arm.pattern)?;
    let guard = arm
        .guard
        .as_ref()
        .map(|e| lower_expression(ctx, e))
        .transpose()?;
    let body = lower_expression(ctx, &arm.body)?;

    Ok(MatchArm {
        pattern,
        guard,
        body,
    })
}

fn lower_pattern(ctx: &mut LoweringContext, pattern: &CstPattern) -> Result<Pattern, ParseError> {
    let kind = match &pattern.kind {
        CstPatternKind::Wildcard => PatternKind::Wildcard,

        CstPatternKind::Binding(ident) => {
            let id = ctx.fresh_binding();
            PatternKind::Binding(id, ident.name.clone())
        }

        CstPatternKind::Literal(lit) => {
            let ast_lit = match lit {
                CstLiteral::Unit => Literal::Unit,
                CstLiteral::Bool(b) => Literal::Bool(*b),
                CstLiteral::Number(n) => Literal::Number(*n),
                CstLiteral::String(s) => Literal::String(s.clone()),
            };
            PatternKind::Literal(ast_lit)
        }

        CstPatternKind::Tuple(elements) => {
            let lowered = elements
                .iter()
                .map(|p| lower_pattern(ctx, p))
                .collect::<Result<Vec<_>, _>>()?;
            PatternKind::Tuple(lowered)
        }

        CstPatternKind::Record(fields) => {
            let lowered = fields
                .iter()
                .map(|f| lower_record_pattern_field(ctx, f))
                .collect::<Result<Vec<_>, _>>()?;
            PatternKind::Record(lowered)
        }

        CstPatternKind::Variant { name, payload } => {
            let lowered_payload = payload
                .as_ref()
                .map(|p| lower_pattern(ctx, p))
                .transpose()?;
            PatternKind::Variant(lower_qualified_name(name), lowered_payload.map(Box::new))
        }

        CstPatternKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error pattern".into()),
                pattern.span,
            ));
        }
    };

    Ok(Pattern::new(kind, pattern.span))
}

fn lower_record_pattern_field(
    ctx: &mut LoweringContext,
    field: &CstRecordPatternField,
) -> Result<(Arc<str>, Pattern), ParseError> {
    let pattern = if let Some(p) = &field.pattern {
        lower_pattern(ctx, p)?
    } else {
        // If no pattern specified, create a binding with the field name
        let id = ctx.fresh_binding();
        Pattern::new(
            PatternKind::Binding(id, field.field.name.clone()),
            field.span,
        )
    };

    Ok((field.field.name.clone(), pattern))
}

fn lower_binary_op(op: CstBinaryOp) -> BinaryOp {
    match op {
        CstBinaryOp::Add => BinaryOp::Add,
        CstBinaryOp::Sub => BinaryOp::Sub,
        CstBinaryOp::Mul => BinaryOp::Mul,
        CstBinaryOp::Div => BinaryOp::Div,
        CstBinaryOp::Mod => BinaryOp::Mod,
        CstBinaryOp::Eq => BinaryOp::Eq,
        CstBinaryOp::Ne => BinaryOp::Ne,
        CstBinaryOp::Lt => BinaryOp::Lt,
        CstBinaryOp::Le => BinaryOp::Le,
        CstBinaryOp::Gt => BinaryOp::Gt,
        CstBinaryOp::Ge => BinaryOp::Ge,
        CstBinaryOp::And => BinaryOp::And,
        CstBinaryOp::Or => BinaryOp::Or,
    }
}

fn lower_unary_op(op: CstUnaryOp) -> UnaryOp {
    match op {
        CstUnaryOp::Neg => UnaryOp::Neg,
        CstUnaryOp::Not => UnaryOp::Not,
    }
}
