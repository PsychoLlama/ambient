//! Expression type inference.
//!
//! This module implements the main `infer_expr` function which infers types for
//! all expression kinds including:
//! - Literals (unit, bool, number, string)
//! - Variables (local and named)
//! - Tuples and records
//! - Lists
//! - Binary and unary operations
//! - Conditionals and pattern matching
//! - Lambdas and function calls
//! - Effect operations (perform, suspend, handle, resume)
//! - Handler literals and sandbox expressions
//!
//! Layout:
//! - `mod.rs`: the `infer_expr` dispatch over `ExprKind`
//! - `calls.rs`: method/associated/inherent call dispatch + `substitute_self`
//! - `effects.rs`: handler literals + sandbox expressions
//! - `operators.rs`: binary operators (built-in and trait-overloaded)

mod calls;
mod effects;
mod operators;
#[cfg(test)]
mod tests;

pub(super) use calls::substitute_self;

use std::sync::Arc;

use super::error::BoxedTypeErrorExt;
use super::{Infer, InferResult, TypeEnv, TypeError, TypeErrorKind, type_error};
use crate::ast::{Expr, ExprKind, StmtKind, UnaryOp};
use crate::types::{LIST_UUID, Type};

/// The element expectation a `List<T>` type carries into a list literal.
fn expected_list_elem(expected: Option<&Type>) -> Option<&Type> {
    match expected {
        Some(Type::Named(n)) if n.uuid == Some(LIST_UUID) && n.args.len() == 1 => Some(&n.args[0]),
        _ => None,
    }
}

impl Infer {
    /// Infer the type of an expression.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if type inference fails.
    pub fn infer_expr(&mut self, env: &TypeEnv, expr: &mut Expr) -> InferResult<Type> {
        self.infer_expr_expecting(env, expr, None)
    }

    /// Infer the type of an expression under an optional *expected* type —
    /// a written annotation (`let`/`const`, a declared return type) pushed
    /// down from the context.
    ///
    /// The expectation is a structural hint, not a check: it seeds the
    /// positions inference would otherwise mint fresh variables for
    /// (unannotated lambda parameters, list elements, a match's result) and
    /// threads through type-transparent frames (blocks, `if`/`match`
    /// branches), so a generic initializer resolves against the annotation
    /// *during* inference and mismatches surface at the sub-expression that
    /// caused them. Leaves ignore it — the caller's definitive
    /// unify-against-annotation still runs after inference returns.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if type inference fails.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn infer_expr_expecting(
        &mut self,
        env: &TypeEnv,
        expr: &mut Expr,
        expected: Option<&Type>,
    ) -> InferResult<Type> {
        // NOTE: module-alias method-call disambiguation (`utils.helper(x)`
        // as a qualified call when `utils` is a module alias) happens in the
        // resolve pass (`crate::resolve`), which runs before checking.

        // Ground the expectation in what earlier inference already solved,
        // so shape matches below see through bound variables.
        let expected = expected.map(|t| self.apply(t));
        let expected = expected.as_ref();

        let span = (expr.span.start, expr.span.end);
        // Split borrows: several arms annotate `dicts` (the dictionary
        // sources of a bounded-generic instantiation) while matching on
        // `kind`.
        let Expr { kind, dicts, .. } = &mut *expr;
        let ty = match kind {
            ExprKind::Unit => Type::Unit,
            ExprKind::Bool(_) => Type::bool(),
            ExprKind::Number(_) => Type::number(),
            ExprKind::String(_) => Type::string(),

            ExprKind::Local(id) => {
                let scheme = env
                    .get(*id)
                    .ok_or_else(|| type_error(TypeErrorKind::UndefinedBinding { id: *id }, span))?;
                self.instantiate(scheme)
            }

            ExprKind::Name(name) => {
                // The resolve pass canonicalized every cross-module
                // reference, and `bind_all_module_exports` bound every
                // public item under its canonical key — so the resolution
                // key is the single lookup convention. Unresolved bare
                // names are locals or module-local items, whose keys are
                // their bare names.
                let scheme = env.get_key(&name.resolution_key());
                let scheme = scheme.ok_or_else(|| {
                    type_error(
                        TypeErrorKind::UndefinedVariable {
                            name: name.joined(),
                        },
                        span,
                    )
                })?;
                let scheme = scheme.clone();
                self.instantiate_bounded(&scheme, span, dicts)
            }

            ExprKind::Tuple(elems) => {
                // An expected tuple of matching arity distributes into the
                // elements.
                let expected_elems = match expected {
                    Some(Type::Tuple(tys)) if tys.len() == elems.len() => Some(tys.clone()),
                    _ => None,
                };
                let mut elem_tys = Vec::with_capacity(elems.len());
                for (i, elem) in elems.iter_mut().enumerate() {
                    let elem_expected = expected_elems.as_ref().map(|tys| &tys[i]);
                    elem_tys.push(self.infer_expr_expecting(env, elem, elem_expected)?);
                }
                Type::Tuple(elem_tys)
            }

            ExprKind::TupleIndex(tuple_expr, idx) => {
                let tuple_ty = self.infer_expr(env, tuple_expr)?;
                let tuple_ty = self.apply(&tuple_ty);
                match &tuple_ty {
                    Type::Tuple(elems) => {
                        let idx_usize = *idx as usize;
                        if idx_usize >= elems.len() {
                            return Err(type_error(
                                TypeErrorKind::TupleIndexOutOfBounds {
                                    index: *idx,
                                    tuple_ty: tuple_ty.clone(),
                                },
                                span,
                            ));
                        }
                        elems[idx_usize].clone()
                    }
                    Type::Var(_) => {
                        // Unknown tuple type, need more context
                        return Err(type_error(
                            TypeErrorKind::CannotInfer {
                                hint: "tuple type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(type_error(
                            TypeErrorKind::TypeMismatch {
                                expected: Type::Tuple(vec![]),
                                actual: tuple_ty,
                            },
                            span,
                        ));
                    }
                }
            }

            ExprKind::Record(fields) => {
                let mut field_tys = Vec::with_capacity(fields.len());
                for (name, expr) in fields {
                    let ty = self.infer_expr(env, expr)?;
                    field_tys.push((name.clone(), ty));
                }
                Type::record(field_tys)
            }

            ExprKind::TypedRecord { type_name, fields } => {
                // Look up the type name by its resolution key: the bare
                // name for local/imported types, the canonical qualified
                // key for path references (`pkg::shapes::Money { … }`).
                let target = self
                    .get_type_alias_key(&type_name.resolution_key())
                    .ok_or_else(|| {
                        type_error(
                            TypeErrorKind::UndefinedTypeName {
                                name: type_name.joined(),
                            },
                            span,
                        )
                    })?;
                // An opaque generic head (`List { … }`) has no record body to
                // construct; it is `extern` by definition, so it fails the
                // same way a nullary `extern` struct does below.
                let Some(full_type) = target.whole().cloned() else {
                    return Err(type_error(
                        TypeErrorKind::CannotConstructExtern {
                            name: type_name.joined(),
                        },
                        span,
                    ));
                };

                // An `extern` struct is engine-provided: user code may name it
                // and read its fields, but not construct it. The `is_extern`
                // flag rides on the nominal identity itself, so it fires the
                // same for a local, imported, or fully-qualified reference.
                if let Type::Nominal(nom) = &full_type
                    && nom.is_extern
                {
                    return Err(type_error(
                        TypeErrorKind::CannotConstructExtern {
                            name: nom.name.clone().unwrap_or_else(|| type_name.joined()),
                        },
                        span,
                    ));
                }

                // Get the expected record type (unwrap Nominal if present)
                let expected_record = match &full_type {
                    Type::Nominal(nom) => match nom.inner.as_ref() {
                        Type::Record(r) => r.clone(),
                        other => {
                            return Err(type_error(
                                TypeErrorKind::NotARecordType { ty: other.clone() },
                                span,
                            ));
                        }
                    },
                    Type::Record(r) => r.clone(),
                    other => {
                        return Err(type_error(
                            TypeErrorKind::NotARecordType { ty: other.clone() },
                            span,
                        ));
                    }
                };

                // Infer field types and unify with expected
                let mut inferred_fields = Vec::with_capacity(fields.len());
                for (name, expr) in fields {
                    let field_ty = self.infer_expr(env, expr)?;
                    inferred_fields.push((name.clone(), field_ty));
                }
                let inferred_record = Type::record(inferred_fields);

                // Unify the inferred record with expected record type
                self.unify(&Type::Record(expected_record), &inferred_record, span)?;

                // Return the full type (Nominal wrapper if applicable)
                full_type
            }

            ExprKind::RecordField(record_expr, field) => {
                let record_ty = self.infer_expr(env, record_expr)?;
                let record_ty = self.apply(&record_ty);

                // Get the record type, unwrapping Nominal if present
                let rec = match &record_ty {
                    Type::Record(rec) => rec,
                    Type::Nominal(nom) => match nom.inner.as_ref() {
                        Type::Record(rec) => rec,
                        _ => {
                            return Err(type_error(
                                TypeErrorKind::FieldNotFound {
                                    field: field.clone(),
                                    record_ty: record_ty.clone(),
                                },
                                span,
                            ));
                        }
                    },
                    Type::Var(_) => {
                        return Err(type_error(
                            TypeErrorKind::CannotInfer {
                                hint: "record type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(type_error(
                            TypeErrorKind::FieldNotFound {
                                field: field.clone(),
                                record_ty: record_ty.clone(),
                            },
                            span,
                        ));
                    }
                };

                rec.get_field(field).cloned().ok_or_else(|| {
                    TypeError::new(
                        TypeErrorKind::FieldNotFound {
                            field: field.clone(),
                            record_ty: record_ty.clone(),
                        },
                        span,
                    )
                })?
            }

            ExprKind::List(elems) => {
                let expected_elem = expected_list_elem(expected).cloned();
                let elem_ty = match expected_elem {
                    Some(ty) => ty,
                    None => self.fresh(),
                };
                for elem in elems {
                    let ty = self.infer_expr_expecting(env, elem, Some(&elem_ty))?;
                    self.unify(&elem_ty, &ty, span)?;
                }
                Type::list(self.apply(&elem_ty))
            }

            ExprKind::Binary {
                op,
                left,
                right,
                resolved_op,
            } => self.infer_binary(env, *op, left, right, resolved_op, span)?,

            ExprKind::Unary(op, operand) => {
                let operand_ty = self.infer_expr(env, operand)?;
                match op {
                    UnaryOp::Neg => {
                        self.unify(&operand_ty, &Type::number(), span)?;
                        Type::number()
                    }
                    UnaryOp::Not => {
                        self.unify(&operand_ty, &Type::bool(), span)?;
                        Type::bool()
                    }
                }
            }

            ExprKind::If(cond, then_branch, else_branch) => {
                let cond_ty = self.infer_expr(env, cond)?;
                self.unify(&cond_ty, &Type::bool(), span)?;

                // Branches share the whole expression's expectation; an
                // else-less `if` is always `()`, so no hint helps it.
                if let Some(else_branch) = else_branch {
                    let then_ty = self.infer_expr_expecting(env, then_branch, expected)?;
                    let else_ty = self.infer_expr_expecting(env, else_branch, expected)?;
                    self.unify(&then_ty, &else_ty, span)?;
                    then_ty
                } else {
                    let then_ty = self.infer_expr(env, then_branch)?;
                    // No else branch means unit type
                    self.unify(&then_ty, &Type::Unit, span)?;
                    Type::Unit
                }
            }

            ExprKind::Match(scrutinee, arms) => {
                let scrutinee_ty = self.infer_expr(env, scrutinee)?;

                if arms.is_empty() {
                    return Err(type_error(
                        TypeErrorKind::CannotInfer {
                            hint: "match expression has no arms".to_string(),
                        },
                        span,
                    ));
                }

                // The expectation seeds the result type, so an off-type arm
                // errors at its own span rather than at the annotation.
                let result_ty = match expected {
                    Some(ty) => ty.clone(),
                    None => self.fresh(),
                };
                for arm in arms.iter_mut() {
                    let arm_env = self.infer_pattern(env, &arm.pattern, &scrutinee_ty)?;
                    // Infer on the real body (not a clone): inference
                    // records resolutions the compiler depends on.
                    let arm_ty = self.infer_expr_expecting(&arm_env, &mut arm.body, expected)?;
                    let arm_span = (arm.body.span.start, arm.body.span.end);
                    self.unify(&result_ty, &arm_ty, arm_span)?;
                }
                self.apply(&result_ty)
            }

            ExprKind::Block(stmts, result) => {
                let mut block_env = env.extend();
                for stmt in stmts {
                    match &mut stmt.kind {
                        StmtKind::Let(binding) => {
                            // The annotation resolves first (an undefined
                            // name in it is reported and rewritten to
                            // `Type::Error`, like a lambda parameter's) so
                            // it can flow into the initializer as its
                            // expected type.
                            let annotated = binding.ty.clone().map(|annotation| {
                                let ann_span = (binding.name_span.start, binding.name_span.end);
                                super::check::resolve_body_annotation(self, &annotation, ann_span)
                            });
                            let init_ty = self.infer_expr_expecting(
                                &block_env,
                                &mut binding.init,
                                annotated.as_ref(),
                            )?;
                            let ty = if let Some(expected) = annotated {
                                let span = (binding.init.span.start, binding.init.span.end);
                                if let Err(e) = self.unify(&expected, &init_ty, span) {
                                    self.pending_errors.push(e.with_context(format!(
                                        "in binding `{}`: type mismatch",
                                        binding.name
                                    )));
                                }
                                // Bind at the annotation even on mismatch:
                                // it's the user's stated intent, and it keeps
                                // downstream errors from cascading.
                                expected
                            } else {
                                init_ty
                            };
                            let scheme = self.generalize(&block_env, &ty);
                            block_env.insert(binding.id, binding.name.clone(), scheme);
                        }
                        StmtKind::Expr(expr) => {
                            self.infer_expr(&block_env, expr)?;
                        }
                        StmtKind::Use(_) => {
                            // Block-scoped imports are a name-resolution
                            // construct consumed by the resolve pass; they
                            // type as nothing and compile to nothing.
                        }
                        StmtKind::Const(const_def) => {
                            // A block `const` binds a name to a literal value,
                            // exactly like a module-level one. Enforce the
                            // literal-only rule, then bind the name at the
                            // const's type — its annotation when written, else
                            // the literal's own type. `const_eval` is the
                            // shared authority on both (kept in lockstep with
                            // the compiler).
                            if crate::const_eval::literal_value(&const_def.value).is_none() {
                                self.pending_errors.push(type_error(
                                    TypeErrorKind::ConstNotLiteral {
                                        name: const_def.name.clone(),
                                    },
                                    (const_def.value.span.start, const_def.value.span.end),
                                ));
                            }
                            // A block `const`'s annotation is a body-local
                            // type: an undefined name in it is reported and
                            // rewritten to `Type::Error`, like a `let`'s.
                            let annotated = const_def.ty.clone().map(|annotation| {
                                let ann_span = (const_def.name_span.start, const_def.name_span.end);
                                super::check::resolve_body_annotation(self, &annotation, ann_span)
                            });
                            let value_ty = self.infer_expr_expecting(
                                &block_env,
                                &mut const_def.value,
                                annotated.as_ref(),
                            )?;
                            let ty = if let Some(expected) = annotated {
                                let span = (const_def.value.span.start, const_def.value.span.end);
                                if let Err(e) = self.unify(&expected, &value_ty, span) {
                                    self.pending_errors.push(e.with_context(format!(
                                        "in constant `{}`: type mismatch",
                                        const_def.name
                                    )));
                                }
                                expected
                            } else {
                                value_ty
                            };
                            block_env.insert(
                                const_def.id,
                                const_def.name.clone(),
                                crate::infer::Scheme::mono(ty),
                            );
                        }
                    }
                }
                if let Some(result) = result {
                    // The block's value is its result expression, so the
                    // expectation passes straight through.
                    self.infer_expr_expecting(&block_env, result, expected)?
                } else {
                    Type::Unit
                }
            }

            ExprKind::Lambda(lambda) => {
                // An expected function type of matching arity seeds the
                // unannotated parameters and the body's expected return —
                // this is what lets `let f: (Stats) -> Number = s => s.hits`
                // type `s` without a written annotation. Written annotations
                // still win (the caller's final unify checks them).
                let expected_fn = match expected {
                    Some(Type::Function(f)) if f.params.len() == lambda.params.len() => {
                        Some(f.clone())
                    }
                    _ => None,
                };

                let mut lambda_env = env.extend();
                let mut param_tys = Vec::with_capacity(lambda.params.len());

                for (i, param) in lambda.params.iter().enumerate() {
                    // Resolve holes in type annotations (e.g., `_` becomes a
                    // fresh variable); an undefined type name is reported and
                    // becomes `Type::Error`.
                    let param_ty = match (&param.ty, &expected_fn) {
                        (Some(ty), _) => super::check::resolve_body_annotation(
                            self,
                            ty,
                            (param.span.start, param.span.end),
                        ),
                        (None, Some(f)) => f.params[i].clone(),
                        (None, None) => self.fresh(),
                    };
                    param_tys.push(param_ty.clone());
                    lambda_env.insert_mono(param.id, param.name.clone(), param_ty);
                }

                // The abilities performed by the lambda's body belong to the
                // lambda's own function type — the enclosing function only
                // requires them if it actually calls the lambda.
                let body_expected = expected_fn.as_ref().map(|f| f.ret.as_ref());
                let (body_result, lambda_abilities) = self.with_isolated_effects(|infer| {
                    infer.infer_expr_expecting(&lambda_env, &mut lambda.body, body_expected)
                });
                let ret_ty = body_result?;

                Type::function_with_abilities(
                    param_tys,
                    ret_ty,
                    self.apply_abilities(&lambda_abilities),
                )
            }

            ExprKind::Call(callee, args) => {
                // Associated trait-function call: `Type::method(args)` where
                // the method takes no `self` (e.g. `Config::default()`).
                // Resolve it to the canonical impl-method symbol and rewrite
                // the callee to reference that symbol directly, so the
                // compiler emits an ordinary call with no receiver.
                let mut associated_ret = None;
                let associated_target = match &callee.kind {
                    ExprKind::Name(name) if name.path.len() == 1 => {
                        Some((Arc::clone(&name.path[0]), Arc::clone(&name.name)))
                    }
                    _ => None,
                };
                if let Some((type_name, method_name)) = associated_target
                    && let Some((symbol, ret_ty)) = self.try_infer_associated_call(
                        env,
                        &type_name,
                        &method_name,
                        args,
                        span,
                        &mut callee.dicts,
                    )?
                {
                    if let ExprKind::Name(name) = &mut callee.kind {
                        name.path.clear();
                        name.name = symbol;
                    }
                    associated_ret = Some(ret_ty);
                }

                if let Some(ret_ty) = associated_ret {
                    ret_ty
                } else {
                    let callee_ty = self.infer_expr(env, callee)?;
                    let mut arg_tys = Vec::with_capacity(args.len());
                    for arg in args {
                        arg_tys.push(self.infer_expr(env, arg)?);
                    }

                    // Expect a function whose abilities are a fresh variable:
                    // unification binds it to the callee's actual ability set
                    // (Empty for pure functions), which the caller then
                    // requires. This is what propagates effects across
                    // function calls.
                    let ret_ty = self.fresh();
                    let ability_var = self.fresh_ability_var();
                    let expected_fn_ty =
                        Type::function_with_abilities(arg_tys, ret_ty.clone(), ability_var.clone());
                    self.unify(&callee_ty, &expected_fn_ty, span)?;

                    let callee_abilities = self.apply_abilities(&ability_var);
                    self.require_abilities(&callee_abilities);
                    self.apply(&ret_ty)
                }
            }

            // Effect expressions are handled by helper methods in effects.rs
            ExprKind::Perform(ability_call) => {
                self.infer_perform(env, ability_call, dicts, span)?
            }

            ExprKind::Handle(handle_expr) => self.infer_handle(env, handle_expr, span)?,

            ExprKind::Resume(value) => {
                // Resume feeds a value to the captured continuation: the
                // value must be what the perform site expects — the ability
                // method's return type. The resume expression itself
                // evaluates to the handled computation's final result (the
                // handle expression's result type), which the enclosing
                // handler-arm context supplies.
                let value_ty = self.infer_expr(env, value)?;
                let Some(ctx) = self.resume_contexts.last().cloned() else {
                    return Err(type_error(TypeErrorKind::ResumeOutsideHandler, span));
                };
                // A never-returning method has no continuation — the
                // perform site unwinds. Checked before the value unifies:
                // never-typed resume arguments adopt fresh variables
                // (bottom elimination), so unification alone would let
                // `resume(rethrow!(...))` slip through.
                if let Some((ability, method)) = ctx.never_method {
                    return Err(type_error(
                        TypeErrorKind::ResumeNeverMethod { ability, method },
                        span,
                    ));
                }
                if let Some(expected) = &ctx.value_ty {
                    self.unify(expected, &value_ty, span).map_err(|e| {
                        e.with_context("resume value must match the handled method's return type")
                    })?;
                }
                match ctx.result_ty {
                    Some(result) => result,
                    None => self.fresh(),
                }
            }

            ExprKind::HandlerLiteral(handler_lit) => {
                self.infer_handler_literal(env, handler_lit, span)?
            }

            ExprKind::Sandbox(sandbox_expr) => self.infer_sandbox(env, sandbox_expr, span)?,

            ExprKind::MethodCall {
                receiver,
                method,
                method_span,
                args,
                resolved_method,
            } => self.infer_method_call(
                env,
                receiver,
                method,
                *method_span,
                args,
                resolved_method,
                dicts,
            )?,
        };

        // Bottom elimination: a never-typed expression adopts a fresh
        // inference variable — the `∀a. a` encoding of divergence. The
        // value can never exist, so the (unreachable) use site may assume
        // any type: `if c { n } else { Exception::throw!("...") }` is a
        // `Number`. `!` is only *introduced* by declared signatures, so
        // annotations stay strict — checking `fn f(): !` unifies the
        // declared `Never` with the body's adopted variable (binding it),
        // while a body producing a real value keeps its concrete type and
        // still fails to unify with `Never`.
        //
        // The expression's *recorded* type stays the honest pre-adoption
        // `!`: the adopted variable belongs to the use site, and when
        // nothing constrains it (`let x = Exception::throw!("boom");`) it
        // would sit in `expr.ty` as an unbound variable — tooling reading
        // the checked AST (hover) would render inference noise instead of
        // the divergence the checker proved.
        if self.is_never(&ty) {
            expr.ty = Some(Type::Never);
            return Ok(self.fresh());
        }
        expr.ty = Some(ty.clone());
        Ok(ty)
    }

    /// Whether a type is `!` — directly, or an inference variable already
    /// bound to it (bottom elimination — see the note at the end of
    /// [`Self::infer_expr`], its only caller).
    fn is_never(&self, ty: &Type) -> bool {
        match ty {
            Type::Never => true,
            Type::Var(_) => matches!(self.apply(ty), Type::Never),
            _ => false,
        }
    }
}
