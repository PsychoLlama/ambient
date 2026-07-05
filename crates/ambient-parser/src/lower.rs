//! CST to AST lowering.
//!
//! This module converts the parser's CST representation to the AST
//! defined in `ambient-engine`, which is used for type checking and
//! compilation.

use std::sync::Arc;

use uuid::Uuid;

use ambient_engine::ast::{
    AbilityCall, AbilityDef, AbilityMethod, BinaryOp, BindingId, ConstDef, EnumDef, EnumVariant,
    Expr, ExprKind, FunctionDef, HandleExpr, Handler, HandlerLiteralExpr, HandlerLiteralMethod,
    ImplDef, ImplMethod, Item, ItemKind, Lambda, LetBinding, Literal, MatchArm, Module, Param,
    Pattern, PatternKind, QualifiedName, SandboxExpr, Span, Stmt, StmtKind, TraitDef, TraitMethod,
    TypeAliasDef, TypeParam, UnaryOp, UseDef, UsePrefix, WhereClause,
};
use ambient_engine::types::{NominalType, Type};

use crate::cst::{
    CstAbilityDef, CstBinaryOp, CstConstDef, CstEnumDef, CstExpr, CstExprKind, CstFunctionDef,
    CstImplDef, CstItem, CstItemKind, CstLambda, CstLiteral, CstMatchArm, CstModule, CstParam,
    CstPattern, CstPatternKind, CstQualifiedName, CstRecordPatternField, CstStmt, CstStmtKind,
    CstTraitDef, CstTraitParamKind, CstTypeAliasDef, CstTypeExpr, CstTypeExprKind, CstUnaryOp,
    CstUseDef, CstUseTree, CstUseTreeKind, StringPart,
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
///
/// # Errors
///
/// Returns a `ParseError` if the CST cannot be lowered to an AST.
pub fn lower_module(cst: &CstModule) -> Result<Module, ParseError> {
    let mut ctx = LoweringContext::new();

    // Extract module-level documentation from inner doc comments (`//!`)
    let doc = cst
        .leading_trivia
        .extract_inner_doc_comments()
        .map(Arc::from);

    let mut items = Vec::new();
    for cst_item in &cst.items {
        items.extend(lower_item_impl(&mut ctx, cst_item)?);
    }

    Ok(Module {
        name: cst.name.clone(),
        doc,
        items,
    })
}

/// Lower a CST module to an AST module, skipping items that fail to lower.
///
/// Companion to `Parser::parse_module_recovering`: each item that cannot be
/// lowered (invalid UUID, missing `unique` on an enum, ...) is dropped and its
/// error collected, so tooling can work with the rest of the module.
pub fn lower_module_recovering(cst: &CstModule) -> (Module, Vec<ParseError>) {
    let mut ctx = LoweringContext::new();

    // Extract module-level documentation from inner doc comments (`//!`)
    let doc = cst
        .leading_trivia
        .extract_inner_doc_comments()
        .map(Arc::from);

    let mut items = Vec::new();
    let mut errors = Vec::new();
    for cst_item in &cst.items {
        match lower_item_impl(&mut ctx, cst_item) {
            Ok(lowered) => items.extend(lowered),
            Err(e) => errors.push(e),
        }
    }

    let module = Module {
        name: cst.name.clone(),
        doc,
        items,
    };
    (module, errors)
}

/// Lower a single expression.
pub fn lower_expr(cst: &CstExpr) -> Result<Expr, ParseError> {
    let mut ctx = LoweringContext::new();
    lower_expression(&mut ctx, cst)
}

/// Lower a single item. A `use` tree flattens to one item per imported
/// leaf; everything else lowers 1:1.
pub fn lower_item(item: &CstItem) -> Result<Vec<Item>, ParseError> {
    let mut ctx = LoweringContext::new();
    lower_item_impl(&mut ctx, item)
}

fn lower_item_impl(ctx: &mut LoweringContext, item: &CstItem) -> Result<Vec<Item>, ParseError> {
    // Extract item documentation from doc comments (`///`)
    let doc = item.leading_trivia.extract_doc_comments().map(Arc::from);

    let kind = match &item.kind {
        CstItemKind::Function(f) => ItemKind::Function(lower_function(ctx, f)?),
        CstItemKind::Const(c) => ItemKind::Const(lower_const(ctx, c)?),
        CstItemKind::TypeAlias(t) => ItemKind::TypeAlias(lower_type_alias(t)?),
        CstItemKind::Enum(e) => ItemKind::Enum(lower_enum(e)?),
        CstItemKind::Ability(a) => ItemKind::Ability(lower_ability_def(a)?),
        CstItemKind::Use(u) => {
            // A use tree flattens to one item per imported leaf.
            return Ok(lower_use(u)?
                .into_iter()
                .map(|use_def| Item::with_doc(ItemKind::Use(use_def), item.span, doc.clone()))
                .collect());
        }
        CstItemKind::Trait(t) => ItemKind::Trait(lower_trait_def(t)?),
        CstItemKind::Impl(i) => ItemKind::Impl(lower_impl_def(ctx, i)?),
        CstItemKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error item".into()),
                item.span,
            ));
        }
    };

    Ok(vec![Item::with_doc(kind, item.span, doc)])
}

fn lower_function(
    ctx: &mut LoweringContext,
    f: &CstFunctionDef,
) -> Result<FunctionDef, ParseError> {
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

    let abilities = f.abilities.iter().map(lower_qualified_name).collect();

    let body = lower_expression(ctx, &f.body)?;

    Ok(FunctionDef {
        name: f.name.name.clone(),
        name_span: f.name.span,
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
        name_span: c.name.span,
        is_public: c.is_public,
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

    // Parse UUID if this is a nominal type
    let unique_id = t
        .unique_id
        .as_ref()
        .map(|s| {
            Uuid::parse_str(s).map_err(|e| {
                ParseError::new(ParseErrorKind::InvalidUuid(e.to_string()), t.name.span)
            })
        })
        .transpose()?;

    let inner_ty = lower_type(&t.ty)?;

    // Wrap in Nominal type if unique_id is present
    let ty = if let Some(uuid) = unique_id {
        Type::Nominal(NominalType::new(uuid, inner_ty, Some(t.name.name.clone())))
    } else {
        inner_ty
    };

    Ok(TypeAliasDef {
        name: t.name.name.clone(),
        name_span: t.name.span,
        is_public: t.is_public,
        type_params,
        ty,
        unique_id,
    })
}

fn lower_enum(e: &CstEnumDef) -> Result<EnumDef, ParseError> {
    // Every enum is nominal: the `unique(<uuid>)` prefix is mandatory. A bare
    // `enum` has no identity, which would make structurally identical enums
    // interchangeable — the exact confusion nominal identity exists to
    // prevent — so reject it here.
    let uuid = match &e.unique_id {
        Some(s) => Uuid::parse_str(s).map_err(|err| {
            ParseError::new(ParseErrorKind::InvalidUuid(err.to_string()), e.name.span)
        })?,
        None => {
            return Err(ParseError::new(
                ParseErrorKind::EnumRequiresUnique,
                e.name.span,
            ));
        }
    };

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
        name_span: e.name.span,
        is_public: e.is_public,
        type_params,
        variants,
        uuid,
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
        name_span: a.name.span,
        is_public: a.is_public,
        dependencies,
        methods,
        resolved_id: None,
    })
}

/// Lower a use tree, flattening it to one `UseDef` per imported leaf.
/// Braces are pure grouping: `use a::{b, c};` lowers exactly like
/// `use a::b; use a::c;`.
fn lower_use(u: &CstUseDef) -> Result<Vec<UseDef>, ParseError> {
    let mut out = Vec::new();
    flatten_use_tree(&u.tree, &[], u.is_public, &mut out)?;
    Ok(out)
}

fn flatten_use_tree(
    tree: &CstUseTree,
    base: &[crate::cst::CstIdent],
    is_public: bool,
    out: &mut Vec<UseDef>,
) -> Result<(), ParseError> {
    let mut full: Vec<crate::cst::CstIdent> = base.to_vec();
    full.extend(tree.segments.iter().cloned());
    match &tree.kind {
        CstUseTreeKind::Leaf { alias } => {
            out.push(lower_use_leaf(&full, alias.as_ref(), is_public, tree.span)?);
        }
        CstUseTreeKind::Group(children) => {
            for child in children {
                flatten_use_tree(child, &full, is_public, out)?;
            }
        }
    }
    Ok(())
}

/// Lower one flattened use path. The head segment determines the root:
/// a keyword (`pkg`, `core`, `self`, `super`, contextual `platform`) or a
/// module alias from another `use` (`UsePrefix::Local`). Root keywords
/// anywhere but the head are errors.
fn lower_use_leaf(
    full: &[crate::cst::CstIdent],
    alias: Option<&crate::cst::CstIdent>,
    is_public: bool,
    span: Span,
) -> Result<UseDef, ParseError> {
    let Some(head) = full.first() else {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError("empty use path".into()),
            span,
        ));
    };

    let (prefix, consumed) = match head.name.as_ref() {
        "pkg" => (UsePrefix::Pkg, 1),
        "core" => (UsePrefix::Core, 1),
        "platform" => (UsePrefix::Platform, 1),
        "self" => (UsePrefix::Self_, 1),
        "super" => {
            let supers = full
                .iter()
                .take_while(|seg| seg.name.as_ref() == "super")
                .count();
            (UsePrefix::Super(supers), supers)
        }
        _ => (UsePrefix::Local, 0),
    };

    for seg in &full[consumed..] {
        if matches!(seg.name.as_ref(), "pkg" | "core" | "self" | "super") {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError(format!("`{}` may only begin a use path", seg.name)),
                seg.span,
            ));
        }
    }

    Ok(UseDef {
        is_public,
        prefix,
        path: full[consumed..]
            .iter()
            .map(|seg| (seg.name.clone(), seg.span))
            .collect(),
        alias: alias.map(|a| (a.name.clone(), a.span)),
    })
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

/// Create a `core::string::concat(left, right)` call expression.
fn make_string_concat_call(left: Expr, right: Expr, span: Span) -> Expr {
    let callee = Expr::new(
        ExprKind::Name(QualifiedName::qualified(
            vec![Arc::from("core"), Arc::from("string")],
            Arc::from("concat"),
        )),
        span,
    );
    Expr::new(ExprKind::Call(Box::new(callee), vec![left, right]), span)
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
        Pattern::new(
            PatternKind::Binding(id, field.field.name.clone()),
            field.span,
        )
    };

    Ok((field.field.name.clone(), pattern))
}

/// Lower a CST type expression to an AST Type.
///
/// # Panics
/// Uses `expect()` for qualified name segments which is safe because qualified
/// names always have at least one segment by parser construction.
#[allow(clippy::expect_used, clippy::too_many_lines)]
fn lower_type(ty: &CstTypeExpr) -> Result<Type, ParseError> {
    match &ty.kind {
        CstTypeExprKind::Name(qn) => {
            let name = type_name_from_segments(qn);
            match &*name {
                "number" => Ok(Type::Number),
                "string" => Ok(Type::String),
                "bool" => Ok(Type::Bool),
                "Bytes" => Ok(Type::Bytes),
                _ => {
                    // Named type - could be generic, user-defined, etc.
                    // For now, represent as a named type
                    Ok(Type::Named(ambient_engine::types::NamedType {
                        name,
                        args: Vec::new(),
                        uuid: None,
                    }))
                }
            }
        }

        CstTypeExprKind::Generic { base, args } => {
            // Extract base name
            let base_name = match &base.kind {
                CstTypeExprKind::Name(qn) => type_name_from_segments(qn),
                _ => {
                    return Err(ParseError::new(ParseErrorKind::InvalidType, base.span));
                }
            };

            let lowered_args = args.iter().map(lower_type).collect::<Result<Vec<_>, _>>()?;

            Ok(Type::Named(ambient_engine::types::NamedType {
                name: base_name,
                args: lowered_args,
                uuid: None,
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
            abilities,
        } => {
            let param_types = params
                .iter()
                .map(lower_type)
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = lower_type(ret)?;

            // Ability names can't be resolved to IDs here (lowering has no
            // ability resolver); pass them through symbolically for the type
            // checker's resolve_holes to resolve. Qualified names keep their
            // full `::`-joined spelling (`platform::Stdio`) so the checker
            // can enforce the namespace policy on effect rows too.
            let ability_set = if abilities.is_empty() {
                ambient_engine::types::AbilitySet::empty()
            } else {
                ambient_engine::types::AbilitySet::Unresolved(
                    abilities
                        .iter()
                        .map(|qn| {
                            let segments: Vec<&str> =
                                qn.segments.iter().map(|seg| seg.name.as_ref()).collect();
                            std::sync::Arc::from(segments.join("::"))
                        })
                        .collect(),
                )
            };

            Ok(Type::Function(ambient_engine::types::FunctionType {
                params: param_types,
                ret: Box::new(return_type),
                abilities: ability_set,
            }))
        }

        CstTypeExprKind::AbilityValue {
            result_ty,
            ability_ty: _,
        } => {
            let result = lower_type(result_ty)?;
            // Ability value type - for now just return the result type
            // Full implementation would track the ability set
            Ok(Type::AbilityValue(
                ambient_engine::types::AbilityValueType {
                    result: Box::new(result),
                    ability: ambient_engine::types::AbilitySet::empty(), // TODO
                },
            ))
        }

        CstTypeExprKind::Never => {
            // Never type - represent as a named type for now
            Ok(Type::Named(ambient_engine::types::NamedType {
                name: "!".into(),
                args: Vec::new(),
                uuid: None,
            }))
        }

        CstTypeExprKind::Infer => {
            // Inferred type - the type checker will fill this in
            // For now, return a type variable placeholder
            Ok(Type::Var(0))
        }

        CstTypeExprKind::Error => Err(ParseError::new(
            ParseErrorKind::LoweringError("cannot lower error type".into()),
            ty.span,
        )),
    }
}

/// The qualified name a type reference lowers to: the bare last segment
/// for an unqualified reference, the full qualified path (`pkg::types::Money`,
/// `core::time::Duration`) for a qualified one. The resolve pass
/// (`ambient_engine::resolve`) rewrites qualified type names to their
/// canonical target.
fn type_name_from_segments(qn: &CstQualifiedName) -> Arc<str> {
    if qn.segments.len() == 1 {
        qn.segments[0].name.clone()
    } else {
        qn.segments
            .iter()
            .map(|seg| seg.name.as_ref())
            .collect::<Vec<_>>()
            .join("::")
            .into()
    }
}

/// Lower a CST qualified name to an AST `QualifiedName`.
///
/// # Panics
/// Uses `expect()` for segment access which is safe because qualified names
/// always have at least one segment by parser construction.
#[allow(clippy::expect_used)]
fn lower_qualified_name(qn: &CstQualifiedName) -> QualifiedName {
    if qn.segments.len() == 1 {
        let seg = &qn.segments[0];
        QualifiedName {
            path: Vec::new(),
            path_spans: Vec::new(),
            name: seg.name.clone(),
            name_span: Some(seg.span),
            resolved: None,
        }
    } else {
        let name_seg = qn.segments.last().expect("segments not empty");
        let path: Vec<Arc<str>> = qn.segments[..qn.segments.len() - 1]
            .iter()
            .map(|i| i.name.clone())
            .collect();
        let path_spans: Vec<Span> = qn.segments[..qn.segments.len() - 1]
            .iter()
            .map(|i| i.span)
            .collect();
        QualifiedName {
            path,
            path_spans,
            name: name_seg.name.clone(),
            name_span: Some(name_seg.span),
            resolved: None,
        }
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

// ─────────────────────────────────────────────────────────────────────────────
// Trait and Impl Lowering
// ─────────────────────────────────────────────────────────────────────────────

fn lower_trait_def(t: &CstTraitDef) -> Result<TraitDef, ParseError> {
    let type_params = t
        .type_params
        .iter()
        .map(|tp| TypeParam {
            name: tp.name.name.clone(),
            span: tp.span,
        })
        .collect();

    let supertraits = t.supertraits.iter().map(lower_qualified_name).collect();

    let methods = t
        .methods
        .iter()
        .map(|m| {
            let method_type_params = m
                .type_params
                .iter()
                .map(|tp| TypeParam {
                    name: tp.name.name.clone(),
                    span: tp.span,
                })
                .collect();

            // Check if first param is self
            let (has_self, other_params) = if let Some(first) = m.params.first() {
                match &first.kind {
                    CstTraitParamKind::SelfParam => (true, &m.params[1..]),
                    CstTraitParamKind::Named { .. } => (false, &m.params[..]),
                }
            } else {
                (false, &m.params[..])
            };

            let params = other_params
                .iter()
                .map(|p| match &p.kind {
                    CstTraitParamKind::Named { name, ty } => {
                        let lowered_ty = lower_type(ty)?;
                        Ok((name.name.clone(), lowered_ty))
                    }
                    CstTraitParamKind::SelfParam => Err(ParseError::new(
                        ParseErrorKind::LoweringError(
                            "self can only be the first parameter".into(),
                        ),
                        p.span,
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;

            let ret_ty = lower_type(&m.ret_ty)?;

            Ok(TraitMethod {
                name: m.name.name.clone(),
                name_span: m.name.span,
                type_params: method_type_params,
                has_self,
                params,
                ret_ty,
                span: m.span,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TraitDef {
        name: t.name.name.clone(),
        name_span: t.name.span,
        is_public: t.is_public,
        type_params,
        supertraits,
        methods,
    })
}

fn lower_impl_def(ctx: &mut LoweringContext, i: &CstImplDef) -> Result<ImplDef, ParseError> {
    let type_params = i
        .type_params
        .iter()
        .map(|tp| TypeParam {
            name: tp.name.name.clone(),
            span: tp.span,
        })
        .collect();

    let trait_name = i.trait_name.as_ref().map(lower_qualified_name);
    let for_type = lower_type(&i.for_type)?;

    let where_clauses = i
        .where_clauses
        .iter()
        .map(|wc| {
            let ty = lower_type(&wc.ty)?;
            let bounds = wc.bounds.iter().map(lower_qualified_name).collect();
            Ok(WhereClause { ty, bounds })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let methods = i
        .methods
        .iter()
        .map(|m| {
            let method_type_params = m
                .type_params
                .iter()
                .map(|tp| TypeParam {
                    name: tp.name.name.clone(),
                    span: tp.span,
                })
                .collect();

            // Allocate self binding ID
            let self_id = ctx.fresh_binding();

            // A leading `self` parameter marks an instance method; its
            // absence marks an associated method (e.g. `Default::default`).
            let has_self = m
                .params
                .first()
                .is_some_and(|p| matches!(p.kind, CstTraitParamKind::SelfParam));

            // Lower non-self parameters
            let params = m
                .params
                .iter()
                .filter_map(|p| match &p.kind {
                    CstTraitParamKind::SelfParam => None,
                    CstTraitParamKind::Named { name, ty } => Some({
                        let lowered_ty = lower_type(ty).ok();
                        Ok(Param {
                            id: ctx.fresh_binding(),
                            name: name.name.clone(),
                            ty: lowered_ty,
                            span: p.span,
                        })
                    }),
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            let ret_ty = m.ret_ty.as_ref().map(lower_type).transpose()?;
            let abilities = m.abilities.iter().map(lower_qualified_name).collect();
            let body = lower_expression(ctx, &m.body)?;

            Ok(ImplMethod {
                name: m.name.name.clone(),
                name_span: m.name.span,
                type_params: method_type_params,
                has_self,
                self_id,
                params,
                ret_ty,
                abilities,
                body,
                span: m.span,
                resolved_symbol: None,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ImplDef {
        type_params,
        trait_name,
        for_type,
        where_clauses,
        methods,
        span: i.span,
    })
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
            ExprKind::Binary {
                op: BinaryOp::Add, ..
            } => {}
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
        let source = "unique(A1B2C3D4-0000-0000-0000-000000000001) enum Maybe<T> { Some(T), None }";
        let module = parse(source).expect("parse error");
        match &module.items[0].kind {
            ItemKind::Enum(e) => {
                assert_eq!(&*e.name, "Maybe");
                assert_eq!(e.type_params.len(), 1);
                assert_eq!(e.variants.len(), 2);
                assert_eq!(e.uuid.to_string(), "a1b2c3d4-0000-0000-0000-000000000001");
            }
            _ => panic!("Expected enum"),
        }
    }

    #[test]
    fn test_lower_bare_enum_rejected() {
        // Every enum must carry a `unique(<uuid>)` prefix; a bare `enum` has
        // no nominal identity and is rejected at lowering.
        let source = "enum Color { Red, Green, Blue }";
        let err = parse(source).expect_err("bare enum should be rejected");
        assert!(matches!(err.kind, ParseErrorKind::EnumRequiresUnique));
    }

    #[test]
    fn test_lower_function_with_doc_comment() {
        let source = "/// Adds two numbers.\nfn add(x: number, y: number): number { x + y }";
        let module = parse(source).expect("parse error");
        assert_eq!(module.items.len(), 1);
        let doc = module.items[0].doc.as_ref().expect("Expected doc comment");
        assert_eq!(&**doc, "Adds two numbers.");
    }

    #[test]
    fn test_lower_module_with_inner_doc() {
        let source = "//! Module documentation.\n\nfn foo() { () }";
        let module = parse(source).expect("parse error");
        let doc = module.doc.as_ref().expect("Expected module doc");
        assert_eq!(&**doc, "Module documentation.");
    }

    #[test]
    fn test_lower_nominal_type() {
        let source = "unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) type UserId { value: string }";
        let module = parse(source).expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            ItemKind::TypeAlias(t) => {
                assert_eq!(&*t.name, "UserId");
                assert!(t.unique_id.is_some());
                let uuid = t.unique_id.unwrap();
                // Source syntax is uppercase; the canonical value is lowercase.
                assert_eq!(uuid.to_string(), "d098767b-4093-4d5c-ba37-ad92aa7b5d98");
                // The type should be wrapped in Nominal
                assert!(matches!(t.ty, Type::Nominal(_)));
            }
            _ => panic!("Expected type alias"),
        }
    }

    #[test]
    fn test_lower_nominal_type_uuid_with_exponent_like_group() {
        // Regression: a UUID whose first hex group is `<digit>E<hex letter>`
        // (here `2EB9553C`) once crashed the lexer, which mistook `2E...` for a
        // malformed scientific-notation literal. It is now lexed as a single
        // `Uuid` token and must validate as a real UUID like any other.
        let source = "unique(2EB9553C-1FDF-46FB-A8B1-F2C5A1CFCA94) type Example { value: string }";
        let module = parse(source).expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            ItemKind::TypeAlias(t) => {
                assert_eq!(&*t.name, "Example");
                let uuid = t.unique_id.expect("expected nominal type");
                assert_eq!(uuid.to_string(), "2eb9553c-1fdf-46fb-a8b1-f2c5a1cfca94");
                assert!(matches!(t.ty, Type::Nominal(_)));
            }
            _ => panic!("Expected type alias"),
        }
    }

    #[test]
    fn test_lower_regular_type_alias() {
        let source = "type Point { x: number, y: number }";
        let module = parse(source).expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            ItemKind::TypeAlias(t) => {
                assert_eq!(&*t.name, "Point");
                assert!(t.unique_id.is_none());
                // The type should NOT be wrapped in Nominal
                assert!(matches!(t.ty, Type::Record(_)));
            }
            _ => panic!("Expected type alias"),
        }
    }

    #[test]
    fn test_lower_invalid_uuid() {
        // Non-UUID content in `unique(...)` is now rejected at parse time (the
        // lexer only produces a `Uuid` token for canonical uppercase UUIDs),
        // so the error is `ExpectedUuid` rather than a lowering `InvalidUuid`.
        let source = "unique(not-a-valid-uuid) type BadId { value: string }";
        let result = parse(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::ExpectedUuid));
    }

    #[test]
    fn test_lower_lowercase_uuid_rejected() {
        // A lowercase UUID is not a UUID literal in Ambient; it must be
        // rejected rather than silently accepted as a non-nominal type.
        let source = "unique(2eb9553c-1fdf-46fb-a8b1-f2c5a1cfca94) type BadId { value: string }";
        let result = parse(source);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ParseErrorKind::ExpectedUuid
        ));
    }
}
