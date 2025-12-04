//! CST to AST lowering.
//!
//! This module converts the parser's CST representation to the AST
//! defined in `ambient-engine`, which is used for type checking and
//! compilation.

use std::sync::Arc;

use ambient_engine::ast::{
    AbilityCall, AbilityDef, AbilityMethod, BinaryOp, BindingId, ConstDef, EnumDef, EnumVariant,
    Expr, ExprKind, FunctionDef, HandleExpr, Handler, HandlerLiteralExpr, HandlerLiteralMethod,
    Item, ItemKind, Lambda, LetBinding, Literal, MatchArm, Module, Param, Pattern, PatternKind,
    QualifiedName, SandboxExpr, Span, Stmt, StmtKind, TypeAliasDef, TypeParam, UnaryOp, UseDef,
    UseImports,
};
use ambient_engine::types::Type;

use crate::cst::{
    CstAbilityDef, CstBinaryOp, CstConstDef, CstEnumDef, CstExpr, CstExprKind, CstFunctionDef,
    CstItem, CstItemKind, CstLambda, CstLiteral, CstMatchArm, CstModule, CstParam, CstPattern,
    CstPatternKind, CstQualifiedName, CstRecordPatternField, CstStmt, CstStmtKind,
    CstTypeAliasDef, CstTypeExpr, CstTypeExprKind, CstUnaryOp, CstUseDef, StringPart,
};
use crate::error::{ParseError, ParseErrorKind};

/// Context for lowering, tracking binding IDs.
struct LoweringContext {
    next_binding_id: BindingId,
}

impl LoweringContext {
    fn new() -> Self {
        Self { next_binding_id: 0 }
    }

    fn fresh_binding(&mut self) -> BindingId {
        let id = self.next_binding_id;
        self.next_binding_id += 1;
        id
    }
}

/// Lower a CST module to an AST module.
pub fn lower_module(cst: &CstModule) -> Result<Module, ParseError> {
    let mut ctx = LoweringContext::new();

    let mut items = Vec::new();
    for cst_item in &cst.items {
        items.push(lower_item(&mut ctx, cst_item)?);
    }

    Ok(Module {
        name: cst.name.clone(),
        items,
    })
}

/// Lower a single expression.
pub fn lower_expr(cst: &CstExpr) -> Result<Expr, ParseError> {
    let mut ctx = LoweringContext::new();
    lower_expression(&mut ctx, cst)
}

fn lower_item(ctx: &mut LoweringContext, item: &CstItem) -> Result<Item, ParseError> {
    let kind = match &item.kind {
        CstItemKind::Function(f) => ItemKind::Function(lower_function(ctx, f)?),
        CstItemKind::Const(c) => ItemKind::Const(lower_const(ctx, c)?),
        CstItemKind::TypeAlias(t) => ItemKind::TypeAlias(lower_type_alias(t)?),
        CstItemKind::Enum(e) => ItemKind::Enum(lower_enum(e)?),
        CstItemKind::Ability(a) => ItemKind::Ability(lower_ability_def(a)?),
        CstItemKind::Use(u) => ItemKind::Use(lower_use(u)?),
        CstItemKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error item".into()),
                item.span,
            ));
        }
    };

    Ok(Item::new(kind, item.span))
}

fn lower_function(ctx: &mut LoweringContext, f: &CstFunctionDef) -> Result<FunctionDef, ParseError> {
    let type_params = f
        .type_params
        .iter()
        .map(|tp| TypeParam {
            name: tp.name.name.clone(),
            span: tp.span,
        })
        .collect();

    let params = f
        .params
        .iter()
        .map(|p| lower_param(ctx, p))
        .collect::<Result<Vec<_>, _>>()?;

    let ret_ty = f.ret_ty.as_ref().map(lower_type).transpose()?;

    let abilities = f
        .abilities
        .iter()
        .map(lower_qualified_name)
        .collect();

    let body = lower_expression(ctx, &f.body)?;

    Ok(FunctionDef {
        name: f.name.name.clone(),
        is_public: f.is_public,
        type_params,
        params,
        ret_ty,
        abilities,
        body,
    })
}

fn lower_const(ctx: &mut LoweringContext, c: &CstConstDef) -> Result<ConstDef, ParseError> {
    let ty = lower_type(&c.ty)?;
    let value = lower_expression(ctx, &c.value)?;

    Ok(ConstDef {
        name: c.name.name.clone(),
        ty,
        value,
    })
}

fn lower_type_alias(t: &CstTypeAliasDef) -> Result<TypeAliasDef, ParseError> {
    let type_params = t
        .type_params
        .iter()
        .map(|tp| TypeParam {
            name: tp.name.name.clone(),
            span: tp.span,
        })
        .collect();

    let ty = lower_type(&t.ty)?;

    Ok(TypeAliasDef {
        name: t.name.name.clone(),
        type_params,
        ty,
    })
}

fn lower_enum(e: &CstEnumDef) -> Result<EnumDef, ParseError> {
    let type_params = e
        .type_params
        .iter()
        .map(|tp| TypeParam {
            name: tp.name.name.clone(),
            span: tp.span,
        })
        .collect();

    let variants = e
        .variants
        .iter()
        .map(|v| {
            let payload = v.payload.as_ref().map(lower_type).transpose()?;
            Ok(EnumVariant {
                name: v.name.name.clone(),
                payload,
                span: v.span,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(EnumDef {
        name: e.name.name.clone(),
        type_params,
        variants,
    })
}

fn lower_ability_def(a: &CstAbilityDef) -> Result<AbilityDef, ParseError> {
    let dependencies = a.dependencies.iter().map(lower_qualified_name).collect();

    let methods = a
        .methods
        .iter()
        .map(|m| {
            let type_params = m
                .type_params
                .iter()
                .map(|tp| TypeParam {
                    name: tp.name.name.clone(),
                    span: tp.span,
                })
                .collect();

            let params = m
                .params
                .iter()
                .map(|(name, ty)| {
                    let lowered_ty = lower_type(ty)?;
                    Ok((name.name.clone(), lowered_ty))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            let ret_ty = lower_type(&m.ret_ty)?;

            Ok(AbilityMethod {
                name: m.name.name.clone(),
                type_params,
                params,
                ret_ty,
                span: m.span,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(AbilityDef {
        name: a.name.name.clone(),
        dependencies,
        methods,
    })
}

fn lower_use(u: &CstUseDef) -> Result<UseDef, ParseError> {
    let path = u.path.iter().map(|i| i.name.clone()).collect();

    let imports = match &u.imports {
        crate::cst::CstUseImports::All => UseImports::All,
        crate::cst::CstUseImports::Items(items) => {
            UseImports::Items(items.iter().map(|i| i.name.clone()).collect())
        }
        crate::cst::CstUseImports::Single => {
            // For single imports, the last path segment is the imported name
            UseImports::Items(vec![u.path.last().map(|i| i.name.clone()).unwrap_or_default()])
        }
    };

    Ok(UseDef { path, imports })
}

fn lower_param(ctx: &mut LoweringContext, p: &CstParam) -> Result<Param, ParseError> {
    let id = ctx.fresh_binding();
    let ty = p.ty.as_ref().map(lower_type).transpose()?;

    Ok(Param {
        id,
        name: p.name.name.clone(),
        ty,
        span: p.span,
    })
}

#[allow(clippy::too_many_lines)]
fn lower_expression(ctx: &mut LoweringContext, expr: &CstExpr) -> Result<Expr, ParseError> {
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
            ExprKind::Binary(ast_op, Box::new(left), Box::new(right))
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
            let lowered_stmts = stmts
                .iter()
                .map(|s| lower_stmt(ctx, s))
                .collect::<Result<Vec<_>, _>>()?;
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
                span: expr.span,
            })
        }

        CstExprKind::Suspend {
            ability,
            method,
            args,
        } => {
            let lowered_args = args
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;
            ExprKind::Suspend(AbilityCall {
                ability: lower_qualified_name(ability),
                method: method.name.clone(),
                args: lowered_args,
                span: expr.span,
            })
        }

        CstExprKind::Handle(handle) => {
            let body = lower_expression(ctx, &handle.body)?;

            // Lower handler values (from `with` clause)
            let handler_values = handle
                .handler_values
                .iter()
                .map(|e| lower_expression(ctx, e))
                .collect::<Result<Vec<_>, _>>()?;

            let handlers = handle
                .handlers
                .iter()
                .map(|h| {
                    let params = h
                        .params
                        .iter()
                        .map(|p| lower_param(ctx, p))
                        .collect::<Result<Vec<_>, _>>()?;
                    let body = lower_expression(ctx, &h.body)?;

                    Ok(Handler {
                        ability: lower_qualified_name(&h.ability),
                        method: h.method.name.clone(),
                        params,
                        body,
                        span: h.span,
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            let else_clause = handle
                .else_clause
                .as_ref()
                .map(|e| lower_expression(ctx, e))
                .transpose()?;

            ExprKind::Handle(HandleExpr {
                body: Box::new(body),
                handler_values,
                handlers,
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
                        method: m.method.name.clone(),
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
    _ctx: &mut LoweringContext,
    parts: &[StringPart],
    span: Span,
) -> Result<ExprKind, ParseError> {
    // For now, we'll convert interpolated strings to a series of
    // string concatenations using to_string and string interpolation
    // In a full implementation, this would be handled specially

    // Simple case: just combine into a single string expression
    // This is a placeholder - real implementation would need runtime support
    let mut combined = String::new();
    for part in parts {
        match part {
            StringPart::Literal(s, _) => combined.push_str(s),
            StringPart::Expr(_) => combined.push_str("${...}"), // Placeholder
        }
    }

    // For now, return a placeholder. In a full implementation,
    // we'd generate proper AST for string concatenation
    if parts.len() == 1 {
        if let StringPart::Literal(s, _) = &parts[0] {
            return Ok(ExprKind::String(s.clone()));
        }
    }

    // Create a more sophisticated representation for interpolation
    // For now, we'll use the Call syntax to represent this
    // In a real implementation, you'd want a dedicated InterpolatedString variant
    // or desugar to function calls

    // Simplified: just return the first string part or create a record
    // representing the interpolation. This is a placeholder.
    Err(ParseError::new(
        ParseErrorKind::LoweringError(
            "interpolated strings not fully implemented in lowering".into(),
        ),
        span,
    ))
}

fn lower_stmt(ctx: &mut LoweringContext, stmt: &CstStmt) -> Result<Stmt, ParseError> {
    let kind = match &stmt.kind {
        CstStmtKind::Let(binding) => {
            let id = ctx.fresh_binding();
            let ty = binding.ty.as_ref().map(lower_type).transpose()?;
            let init = lower_expression(ctx, &binding.init)?;

            StmtKind::Let(LetBinding {
                id,
                name: binding.name.name.clone(),
                ty,
                init,
            })
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

    Ok(Stmt::new(kind, stmt.span))
}

fn lower_lambda(ctx: &mut LoweringContext, lambda: &CstLambda) -> Result<Lambda, ParseError> {
    let params = lambda
        .params
        .iter()
        .map(|p| lower_param(ctx, p))
        .collect::<Result<Vec<_>, _>>()?;

    let ret_ty = lambda.ret_ty.as_ref().map(lower_type).transpose()?;
    let body = lower_expression(ctx, &lambda.body)?;

    Ok(Lambda {
        params,
        ret_ty,
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
        Pattern::new(PatternKind::Binding(id, field.field.name.clone()), field.span)
    };

    Ok((field.field.name.clone(), pattern))
}

#[allow(clippy::expect_used, clippy::too_many_lines)]
fn lower_type(ty: &CstTypeExpr) -> Result<Type, ParseError> {
    match &ty.kind {
        CstTypeExprKind::Name(qn) => {
            // SAFETY: Qualified names always have at least one segment by construction
            let name = &qn.segments.last().expect("qualified name must have segments").name;
            match &**name {
                "number" => Ok(Type::Number),
                "string" => Ok(Type::String),
                "bool" => Ok(Type::Bool),
                _ => {
                    // Named type - could be generic, user-defined, etc.
                    // For now, represent as a named type
                    Ok(Type::Named(ambient_engine::types::NamedType {
                        name: name.clone(),
                        args: Vec::new(),
                    }))
                }
            }
        }

        CstTypeExprKind::Generic { base, args } => {
            // Extract base name
            let base_name = match &base.kind {
                CstTypeExprKind::Name(qn) => {
                    qn.segments.last().expect("qualified name must have segments").name.clone()
                }
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::InvalidType,
                        base.span,
                    ));
                }
            };

            let lowered_args = args
                .iter()
                .map(lower_type)
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Type::Named(ambient_engine::types::NamedType {
                name: base_name,
                args: lowered_args,
            }))
        }

        CstTypeExprKind::Tuple(elements) => {
            if elements.is_empty() {
                Ok(Type::Unit)
            } else {
                let lowered = elements
                    .iter()
                    .map(lower_type)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Type::Tuple(lowered))
            }
        }

        CstTypeExprKind::Record(fields) => {
            let lowered_fields = fields
                .iter()
                .map(|(name, ty)| {
                    let lowered_ty = lower_type(ty)?;
                    Ok((name.name.clone(), lowered_ty))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            Ok(Type::Record(ambient_engine::types::RecordType {
                fields: lowered_fields.into_iter().collect(),
            }))
        }

        CstTypeExprKind::Function {
            params,
            ret,
            abilities: _,
        } => {
            let param_types = params
                .iter()
                .map(lower_type)
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = lower_type(ret)?;

            Ok(Type::Function(ambient_engine::types::FunctionType {
                params: param_types,
                ret: Box::new(return_type),
                abilities: ambient_engine::types::AbilitySet::empty(), // TODO: lower abilities
            }))
        }

        CstTypeExprKind::AbilityValue {
            result_ty,
            ability_ty: _,
        } => {
            let result = lower_type(result_ty)?;
            // Ability value type - for now just return the result type
            // Full implementation would track the ability set
            Ok(Type::AbilityValue(ambient_engine::types::AbilityValueType {
                result: Box::new(result),
                ability: ambient_engine::types::AbilitySet::empty(), // TODO
            }))
        }

        CstTypeExprKind::Never => {
            // Never type - represent as a named type for now
            Ok(Type::Named(ambient_engine::types::NamedType {
                name: "!".into(),
                args: Vec::new(),
            }))
        }

        CstTypeExprKind::Infer => {
            // Inferred type - the type checker will fill this in
            // For now, return a type variable placeholder
            Ok(Type::Var(ambient_engine::types::TypeVar::Unbound(0)))
        }

        CstTypeExprKind::Error => {
            Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error type".into()),
                ty.span,
            ))
        }
    }
}

#[allow(clippy::expect_used)]
fn lower_qualified_name(qn: &CstQualifiedName) -> QualifiedName {
    // SAFETY: Qualified names always have at least one segment by construction
    let segments: Vec<Arc<str>> = qn.segments.iter().map(|i| i.name.clone()).collect();
    if segments.len() == 1 {
        QualifiedName::simple(segments.into_iter().next().expect("segments not empty"))
    } else {
        let name = segments.last().expect("segments not empty").clone();
        let path = segments[..segments.len() - 1].to_vec();
        QualifiedName { path, name }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn test_lower_simple_function() {
        let source = "fn add(x: number, y: number): number { x + y }";
        let module = parse(source).expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            ItemKind::Function(f) => {
                assert_eq!(&*f.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_expression() {
        use crate::parse_expr;

        let expr = parse_expr("1 + 2 * 3").expect("parse error");
        match expr.kind {
            ExprKind::Binary(BinaryOp::Add, _, _) => {}
            _ => panic!("Expected binary add expression"),
        }
    }

    #[test]
    fn test_lower_if_expression() {
        use crate::parse_expr;

        let expr = parse_expr("if true { 1 } else { 2 }").expect("parse error");
        match expr.kind {
            ExprKind::If(cond, _then_br, else_br) => {
                assert!(matches!(cond.kind, ExprKind::Bool(true)));
                assert!(else_br.is_some());
            }
            _ => panic!("Expected if expression"),
        }
    }

    #[test]
    fn test_lower_lambda() {
        use crate::parse_expr;

        let expr = parse_expr("(x) => x + 1").expect("parse error");
        match expr.kind {
            ExprKind::Lambda(lambda) => {
                assert_eq!(lambda.params.len(), 1);
                assert_eq!(&*lambda.params[0].name, "x");
            }
            _ => panic!("Expected lambda"),
        }
    }

    #[test]
    fn test_lower_record() {
        use crate::parse_expr;

        let expr = parse_expr("{ x: 1, y: 2 }").expect("parse error");
        match expr.kind {
            ExprKind::Record(fields) => {
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("Expected record"),
        }
    }

    #[test]
    fn test_lower_enum() {
        let source = "enum Option<T> { Some(T), None }";
        let module = parse(source).expect("parse error");
        match &module.items[0].kind {
            ItemKind::Enum(e) => {
                assert_eq!(&*e.name, "Option");
                assert_eq!(e.type_params.len(), 1);
                assert_eq!(e.variants.len(), 2);
            }
            _ => panic!("Expected enum"),
        }
    }
}
