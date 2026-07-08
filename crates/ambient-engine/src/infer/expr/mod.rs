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

use super::error::BoxedTypeErrorExt;
use super::{Infer, InferResult, TypeEnv, TypeError, TypeErrorKind, type_error};
use crate::ast::{Expr, ExprKind, StmtKind, UnaryOp};
use crate::types::Type;

impl Infer {
    /// Infer the type of an expression.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if type inference fails.
    #[allow(clippy::too_many_lines)]
    pub fn infer_expr(&mut self, env: &TypeEnv, expr: &mut Expr) -> InferResult<Type> {
        // NOTE: module-alias method-call disambiguation (`utils.helper(x)`
        // as a qualified call when `utils` is a module alias) happens in the
        // resolve pass (`crate::resolve`), which runs before checking.

        let span = (expr.span.start, expr.span.end);
        let ty = match &mut expr.kind {
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
                self.instantiate(scheme)
            }

            ExprKind::Tuple(elems) => {
                let mut elem_tys = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_tys.push(self.infer_expr(env, elem)?);
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
                // Look up the type alias by its resolution key: the bare
                // name for local/imported types, the canonical qualified
                // key for path references (`pkg::shapes::Money { … }`).
                let type_alias = self
                    .get_type_alias_key(&type_name.resolution_key())
                    .ok_or_else(|| {
                        type_error(
                            TypeErrorKind::UndefinedTypeName {
                                name: type_name.joined(),
                            },
                            span,
                        )
                    })?;
                let full_type = type_alias.clone();

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
                let elem_ty = self.fresh();
                for elem in elems {
                    let ty = self.infer_expr(env, elem)?;
                    self.unify(&elem_ty, &ty, span)?;
                }
                Type::named("List", vec![self.apply(&elem_ty)])
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

                let then_ty = self.infer_expr(env, then_branch)?;

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(env, else_branch)?;
                    self.unify(&then_ty, &else_ty, span)?;
                    then_ty
                } else {
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

                let result_ty = self.fresh();
                for arm in arms.iter_mut() {
                    let arm_env = self.infer_pattern(env, &arm.pattern, &scrutinee_ty)?;
                    // Infer on the real body (not a clone): inference
                    // records resolutions the compiler depends on.
                    let arm_ty = self.infer_expr(&arm_env, &mut arm.body)?;
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
                            // A `let` annotation isn't yet constrained against
                            // `init` (out of scope), but an undefined type name
                            // in it is still reported (and rewritten away).
                            if let Some(ty) = binding.ty.clone() {
                                let span = (binding.name_span.start, binding.name_span.end);
                                let _ = super::check::resolve_body_annotation(self, &ty, span);
                            }
                            let init_ty = self.infer_expr(&block_env, &mut binding.init)?;
                            let scheme = self.generalize(&block_env, &init_ty);
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
                            let value_ty = self.infer_expr(&block_env, &mut const_def.value)?;
                            let ty = if let Some(annotation) = const_def.ty.clone() {
                                // A block `const`'s annotation is a body-local
                                // type: an undefined name in it is reported and
                                // rewritten to `Type::Error`, like a `let`'s.
                                let ann_span = (const_def.name_span.start, const_def.name_span.end);
                                let expected = super::check::resolve_body_annotation(
                                    self,
                                    &annotation,
                                    ann_span,
                                );
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
                    self.infer_expr(&block_env, result)?
                } else {
                    Type::Unit
                }
            }

            ExprKind::Lambda(lambda) => {
                let mut lambda_env = env.extend();
                let mut param_tys = Vec::with_capacity(lambda.params.len());

                for param in &lambda.params {
                    // Resolve holes in type annotations (e.g., `_` becomes a
                    // fresh variable); an undefined type name is reported and
                    // becomes `Type::Error`.
                    let param_ty = match &param.ty {
                        Some(ty) => super::check::resolve_body_annotation(
                            self,
                            ty,
                            (param.span.start, param.span.end),
                        ),
                        None => self.fresh(),
                    };
                    param_tys.push(param_ty.clone());
                    lambda_env.insert_mono(param.id, param.name.clone(), param_ty);
                }

                // The abilities performed by the lambda's body belong to the
                // lambda's own function type — the enclosing function only
                // requires them if it actually calls the lambda.
                let (body_result, lambda_abilities) = self
                    .with_isolated_effects(|infer| infer.infer_expr(&lambda_env, &mut lambda.body));
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
                if let ExprKind::Name(name) = &callee.kind
                    && name.path.len() == 1
                    && let Some((symbol, ret_ty)) =
                        self.try_infer_associated_call(env, &name.path[0], &name.name, args, span)?
                {
                    if let ExprKind::Name(name) = &mut callee.kind {
                        name.path.clear();
                        name.name = symbol;
                    }
                    return Ok(ret_ty);
                }

                let callee_ty = self.infer_expr(env, callee)?;
                let mut arg_tys = Vec::with_capacity(args.len());
                for arg in args {
                    arg_tys.push(self.infer_expr(env, arg)?);
                }

                // Expect a function whose abilities are a fresh variable:
                // unification binds it to the callee's actual ability set
                // (Empty for pure functions), which the caller then requires.
                // This is what propagates effects across function calls.
                let ret_ty = self.fresh();
                let ability_var = self.fresh_ability_var();
                let expected_fn_ty =
                    Type::function_with_abilities(arg_tys, ret_ty.clone(), ability_var.clone());
                self.unify(&callee_ty, &expected_fn_ty, span)?;

                let callee_abilities = self.apply_abilities(&ability_var);
                self.require_abilities(&callee_abilities);
                self.apply(&ret_ty)
            }

            // Effect expressions are handled by helper methods in effects.rs
            ExprKind::Perform(ability_call) => self.infer_perform(env, ability_call, span)?,

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
            } => {
                self.infer_method_call(env, receiver, method, *method_span, args, resolved_method)?
            }
        };

        expr.ty = Some(ty.clone());
        Ok(ty)
    }
}
