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

use std::sync::Arc;

use super::error::BoxedTypeErrorExt;
use super::{Infer, InferResult, TypeEnv, TypeError, TypeErrorKind, type_error};
use crate::ast::{BinaryOp, Expr, ExprKind, StmtKind, UnaryOp};
use crate::types::{AbilitySet, Type};

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
                let scheme = env.get_by_name(&name.resolution_key());
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
                    .get_type_alias(&type_name.resolution_key())
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
                    // Resolve holes in type annotations (e.g., `_` becomes a fresh variable)
                    let param_ty = match &param.ty {
                        Some(ty) => self.resolve_holes(ty),
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
                // Check for intrinsic functions first
                if let ExprKind::Name(name) = &callee.kind
                    && let Some(ret_ty) = self.try_infer_intrinsic(env, name, args, span)?
                {
                    return Ok(ret_ty);
                }

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

    /// Infer the type of a handler literal expression.
    fn infer_handler_literal(
        &mut self,
        env: &TypeEnv,
        handler_lit: &mut crate::ast::HandlerLiteralExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        use std::sync::Arc;

        // Handler literal type checking (Milestone 13)
        //
        // 1. Collect method names from the handler literal
        // 2. Infer which ability this handler is for (from method names)
        // 3. Verify each method matches the ability's signature
        // 4. Type-check each handler body in an appropriate environment
        // 5. Return Handler<A> type

        // Collect method names
        let method_names: Vec<Arc<str>> = handler_lit
            .methods
            .iter()
            .map(|m| m.method.clone())
            .collect();

        // Try to infer the target ability from method names
        let ability_id = self
            .infer_ability_from_methods(&method_names)
            .ok_or_else(|| {
                TypeError::new(
                    TypeErrorKind::HandlerAbilityAmbiguous {
                        method_names: method_names.clone(),
                    },
                    span,
                )
            })?;

        let ability_name: Arc<str> = self
            .ability_id_to_name(ability_id)
            .unwrap_or("unknown")
            .into();

        // Verify each method in the handler against the ability's declared
        // signature: parameters take the declared types, and `resume` must
        // be fed the method's return type (that is what the perform site
        // receives). The arm body's own value is unconstrained here — it
        // becomes the handle expression's result only at the handle site,
        // which is unknown when a handler value is built.
        for method in &mut handler_lit.methods {
            let method_span = (method.span.start, method.span.end);
            let Some((param_tys, ret_ty)) =
                self.ability_method_signature(ability_id, &method.method)
            else {
                return Err(type_error(
                    TypeErrorKind::HandlerUnknownMethod {
                        ability: ability_name.clone(),
                        method: method.method.clone(),
                    },
                    method_span,
                ));
            };

            if method.params.len() != param_tys.len() {
                return Err(type_error(
                    TypeErrorKind::HandlerMethodArityMismatch {
                        ability: ability_name.clone(),
                        method: method.method.clone(),
                        expected: param_tys.len(),
                        actual: method.params.len(),
                    },
                    method_span,
                ));
            }

            let mut method_env = env.extend();
            for (param, declared_ty) in method.params.iter().zip(&param_tys) {
                let param_ty = match &param.ty {
                    Some(ty) => {
                        let annotated = self.resolve_holes(ty);
                        self.unify(declared_ty, &annotated, method_span)?;
                        annotated
                    }
                    None => declared_ty.clone(),
                };
                method_env.insert_mono(param.id, param.name.clone(), param_ty);
            }

            // A `!`-returning method (Exception::throw) has no statically
            // knowable resume type: the host can raise it at any perform
            // site, and resuming substitutes a value for the failing call.
            let value_ty = match self.apply(&ret_ty) {
                Type::Never => None,
                ret => Some(ret),
            };
            self.resume_contexts.push(crate::infer::ResumeContext {
                value_ty,
                result_ty: None,
            });
            let body_result = self.infer_expr(&method_env, &mut method.body);
            self.resume_contexts.pop();
            body_result?;
        }

        // Handlers don't need to provide all methods - partial handlers are allowed
        // (they can be composed with other handlers that provide missing methods)

        Ok(Type::handler(ability_id))
    }

    /// Infer the type of a sandbox expression.
    ///
    /// A sandbox restricts the abilities the body may *use* to the allowed
    /// list. It installs no handlers — the body executes against the
    /// enclosing context's handlers (the compiler emits the body directly)
    /// — so the body's effects still flow to the enclosing context. The
    /// restriction check runs here when the body's effect set is already
    /// concrete, and is deferred to the end of the module check otherwise
    /// (calls to functions whose effects bind later).
    fn infer_sandbox(
        &mut self,
        env: &TypeEnv,
        sandbox_expr: &mut crate::ast::SandboxExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Resolve the allowed list; unknown names are errors, and the
        // namespace policy applies: `sandbox with platform::Log { ... }`.
        let mut allowed_ability_ids = Vec::with_capacity(sandbox_expr.allowed_abilities.len());
        for ability_name in &sandbox_expr.allowed_abilities {
            allowed_ability_ids.push(self.resolve_ability_ref(ability_name, span)?);
        }

        // Infer the body with a clean accumulator so its effect set can be
        // inspected in isolation — on the real node, not a clone:
        // inference records resolutions (trait method symbols, module-call
        // rewrites, expression types) that the compiler depends on.
        let (body_result, body_abilities) =
            self.with_isolated_effects(|infer| infer.infer_expr(env, &mut sandbox_expr.body));
        let body_ty = body_result?;

        // Enforce the restriction now if the body's effects are already
        // concrete; otherwise defer until every body is checked.
        let allowed_names: Vec<Arc<str>> = sandbox_expr
            .allowed_abilities
            .iter()
            .map(|n| n.name.clone())
            .collect();
        let applied = self.apply_abilities(&body_abilities);
        match &applied {
            AbilitySet::Empty | AbilitySet::Concrete(_) => {
                if let Some(err) = self.sandbox_violation(
                    applied.concrete_abilities(),
                    &allowed_ability_ids,
                    &allowed_names,
                    span,
                ) {
                    return Err(err);
                }
            }
            _ => self
                .pending_sandbox_checks
                .push(crate::infer::PendingSandboxCheck {
                    body: applied.clone(),
                    allowed: allowed_ability_ids,
                    allowed_names,
                    span,
                }),
        }

        // The sandbox installs no handlers, so the body's effects are the
        // enclosing context's problem exactly as if the sandbox weren't
        // there.
        self.require_abilities(&applied);

        Ok(body_ty)
    }

    /// The error for the first body ability outside the allowed list, if
    /// any.
    pub(crate) fn sandbox_violation(
        &self,
        used: &[crate::types::AbilityId],
        allowed: &[crate::types::AbilityId],
        allowed_names: &[Arc<str>],
        span: (u32, u32),
    ) -> Option<super::error::BoxedTypeError> {
        let violation = used.iter().find(|a| !allowed.contains(a))?;
        let ability_name = self.ability_id_to_name(*violation).unwrap_or("unknown");
        Some(type_error(
            TypeErrorKind::SandboxAbilityViolation {
                ability: ability_name.into(),
                allowed: allowed_names.to_vec(),
            },
            span,
        ))
    }

    /// Infer the type of a binary operation.
    ///
    /// For primitive types, uses built-in operators.
    /// For nominal types, looks up the appropriate trait (Add, Eq, etc.).
    #[allow(clippy::too_many_arguments)]
    fn infer_binary(
        &mut self,
        env: &TypeEnv,
        op: BinaryOp,
        left: &mut Expr,
        right: &mut Expr,
        resolved_op: &mut Option<Arc<str>>,
        span: (u32, u32),
    ) -> InferResult<Type> {
        let left_ty = self.infer_expr(env, left)?;
        let right_ty = self.infer_expr(env, right)?;

        // Apply substitutions to get the actual types
        let left_ty = self.apply(&left_ty);
        let right_ty = self.apply(&right_ty);

        // Check for operator overloading on nominal types
        if let Type::Nominal(nominal) = &left_ty {
            // Get the trait and method name for this operator
            if let Some((trait_name, method_name)) = operator_trait(op) {
                // Look up the trait
                if let Some(trait_id) = self.trait_registry.lookup_trait(trait_name) {
                    // Check if the type implements this trait
                    let method_symbol = self
                        .trait_registry
                        .get_impl(trait_id, nominal.uuid)
                        .and_then(|impl_| impl_.methods.get(method_name).cloned());

                    if let Some(symbol) = method_symbol {
                        // Unify operands (both must be the same nominal type)
                        self.unify(&left_ty, &right_ty, span)?;

                        // Store the resolved dispatch symbol for compilation
                        *resolved_op = Some(symbol);

                        // Return type depends on the operator category
                        return Ok(operator_return_type(op, &left_ty));
                    }
                }
            }
        }

        // Built-in operators for primitive types
        match op {
            // Arithmetic operators: Number -> Number -> Number
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                // Special case: Add also works for String concatenation
                if op == BinaryOp::Add && left_ty == Type::string() {
                    self.unify(&right_ty, &Type::string(), span)?;
                    return Ok(Type::string());
                }
                self.unify(&left_ty, &Type::number(), span)?;
                self.unify(&right_ty, &Type::number(), span)?;
                Ok(Type::number())
            }

            // Comparison operators: a -> a -> Bool
            BinaryOp::Eq | BinaryOp::Ne => {
                self.unify(&left_ty, &right_ty, span)?;
                Ok(Type::bool())
            }

            // Ordering operators: Number -> Number -> Bool
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                self.unify(&left_ty, &Type::number(), span)?;
                self.unify(&right_ty, &Type::number(), span)?;
                Ok(Type::bool())
            }

            // Logical operators: Bool -> Bool -> Bool
            BinaryOp::And | BinaryOp::Or => {
                self.unify(&left_ty, &Type::bool(), span)?;
                self.unify(&right_ty, &Type::bool(), span)?;
                Ok(Type::bool())
            }
        }
    }

    /// Type-check a call to an inherent method against its instantiated
    /// scheme. `receiver_ty` is `Some` for dot calls (unified with parameter
    /// 0, which binds the impl's type parameters) and `None` for associated
    /// `Type::method(...)` calls.
    #[allow(clippy::too_many_arguments)]
    fn infer_inherent_call(
        &mut self,
        env: &TypeEnv,
        method: &crate::infer::inherent::InherentMethod,
        receiver_ty: Option<&Type>,
        args: &mut [Expr],
        span: (u32, u32),
        resolved_method: &mut Option<Arc<str>>,
    ) -> InferResult<Type> {
        let fn_ty = self.instantiate(&method.scheme);
        let Type::Function(f) = fn_ty else {
            return Err(type_error(TypeErrorKind::NotAFunction { ty: fn_ty }, span));
        };

        let receiver_count = usize::from(receiver_ty.is_some());
        let expected_args = f.params.len() - receiver_count;
        if args.len() != expected_args {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: expected_args,
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in call to method `{}`", method.name)));
        }

        if let Some(receiver) = receiver_ty {
            self.unify(receiver, &f.params[0], span)
                .map_err(|e| e.with_context(format!("in receiver of method `{}`", method.name)))?;
        }

        for (i, (arg, param_ty)) in args
            .iter_mut()
            .zip(f.params[receiver_count..].iter())
            .enumerate()
        {
            let arg_ty = self.infer_expr(env, arg)?;
            if let Err(e) = self.unify(&arg_ty, param_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of method `{}`",
                    i + 1,
                    method.name
                )));
            }
        }

        // The scheme's ability set is the method's declared effects; the
        // caller must provide them, exactly as for an ordinary call.
        let abilities = self.apply_abilities(&f.abilities);
        self.require_abilities(&abilities);

        *resolved_method = Some(Arc::clone(&method.symbol));
        Ok(self.apply(&f.ret))
    }

    /// Try to resolve a `Type::method(args)` associated-function call.
    ///
    /// Returns `Some((symbol, return_type))` when `type_name` names a type
    /// with a no-`self` method: an inherent associated method (checked
    /// first), or a trait associated method such as `Default::default`
    /// (nominal types only). Returns `None` when this is not such a call —
    /// the caller then falls back to ordinary qualified name resolution, so
    /// module companion functions like `Option::map(opt, f)` keep resolving
    /// to `core::Option::map`. Argument type errors surface as `Err`.
    fn try_infer_associated_call(
        &mut self,
        env: &TypeEnv,
        type_name: &str,
        method_name: &str,
        args: &mut [Expr],
        span: (u32, u32),
    ) -> InferResult<Option<(Arc<str>, Type)>> {
        use crate::infer::inherent::ImplKey;

        // Resolve the leading segment to an impl-target key: a nominal type
        // alias, or an enum / built-in container head.
        let key = if let Some(Type::Nominal(n)) = self.get_type_alias(type_name) {
            Some(ImplKey::Nominal(n.uuid))
        } else if let Some(info) = self.enum_registry.get(type_name) {
            // A declared enum keys on its uuid (matching its receiver-form
            // dispatch); prelude enums key on their reserved head name.
            Some(match info.uuid {
                Some(uuid) => ImplKey::Nominal(uuid),
                None => ImplKey::Named(type_name.into()),
            })
        } else if matches!(type_name, "List" | "Map" | "Set") {
            Some(ImplKey::Named(type_name.into()))
        } else {
            None
        };

        // Inherent associated method?
        if let Some(key) = &key
            && let Some(method) = self.inherent_registry.get(key, method_name)
            && !method.has_self
        {
            let method = method.clone();
            let mut resolved = None;
            let ret = self.infer_inherent_call(env, &method, None, args, span, &mut resolved)?;
            return Ok(resolved.map(|symbol| (symbol, ret)));
        }

        // The leading segment must name a nominal type.
        let Some(Type::Nominal(nominal)) = self.get_type_alias(type_name).cloned() else {
            return Ok(None);
        };

        // The method must exist and be associated (no `self`); an instance
        // method reached this way is not a valid associated call.
        let (params, ret, symbol) = match self.trait_registry.find_method(nominal.uuid, method_name)
        {
            crate::types::MethodLookup::Found { method, symbol, .. } if !method.has_self => {
                (method.params.clone(), method.ret.clone(), symbol)
            }
            _ => return Ok(None),
        };

        let self_ty = Type::Nominal(nominal);

        let mut arg_tys = Vec::with_capacity(args.len());
        for arg in args.iter_mut() {
            arg_tys.push(self.infer_expr(env, arg)?);
        }

        if arg_tys.len() != params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: params.len(),
                    actual: arg_tys.len(),
                },
                span,
            )
            .with_context(format!("in associated call `{type_name}::{method_name}`")));
        }

        for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(params.iter()).enumerate() {
            let param_ty = substitute_self(param_ty, &self_ty);
            if let Err(e) = self.unify(arg_ty, &param_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of associated call `{type_name}::{method_name}`",
                    i + 1
                )));
            }
        }

        Ok(Some((symbol, substitute_self(&ret, &self_ty))))
    }

    /// Infer the type of a method call expression.
    ///
    /// Resolution order: inherent methods first (any type with an impl-key
    /// identity — nominal, enum, built-in container, primitive), then trait
    /// methods (nominal receivers only). Inherent methods shadow same-named
    /// trait methods, so adding an inherent method is a deliberate, local
    /// override — never silent ambiguity.
    #[allow(clippy::too_many_arguments)]
    fn infer_method_call(
        &mut self,
        env: &TypeEnv,
        receiver: &mut Expr,
        method_name: &Arc<str>,
        method_span: crate::ast::Span,
        args: &mut [Expr],
        resolved_method: &mut Option<Arc<str>>,
    ) -> InferResult<Type> {
        // Infer the receiver type
        let receiver_ty = self.infer_expr(env, receiver)?;
        let receiver_ty = self.apply(&receiver_ty);
        let span = (method_span.start, method_span.end);

        // Inherent methods first.
        if let Some(key) = crate::infer::inherent::impl_key_for(&receiver_ty)
            && let Some(method) = self.inherent_registry.get(&key, method_name)
            && method.has_self
        {
            let method = method.clone();
            return self.infer_inherent_call(
                env,
                &method,
                Some(&receiver_ty),
                args,
                span,
                resolved_method,
            );
        }

        // Check if the receiver is a nominal type
        let Type::Nominal(nominal) = &receiver_ty else {
            return Err(type_error(
                TypeErrorKind::MethodNotFound {
                    method: Arc::clone(method_name),
                    ty: receiver_ty.clone(),
                },
                span,
            ));
        };

        // Look up the method in the trait registry
        let (trait_id, method_def, method_symbol) =
            match self.trait_registry.find_method(nominal.uuid, method_name) {
                crate::types::MethodLookup::Found {
                    trait_id,
                    method,
                    symbol,
                } => (trait_id, method, symbol),
                crate::types::MethodLookup::NotFound => {
                    return Err(type_error(
                        TypeErrorKind::MethodNotFound {
                            method: Arc::clone(method_name),
                            ty: receiver_ty.clone(),
                        },
                        span,
                    ));
                }
                crate::types::MethodLookup::Ambiguous { traits } => {
                    return Err(type_error(
                        TypeErrorKind::AmbiguousMethod {
                            method: Arc::clone(method_name),
                            ty: receiver_ty.clone(),
                            candidates: traits,
                        },
                        span,
                    ));
                }
            };

        // Clone the method definition to release the borrow on trait_registry
        let method_def = method_def.clone();

        // Store the resolved dispatch symbol for compilation
        *resolved_method = Some(method_symbol);

        // Infer argument types
        let mut arg_tys = Vec::new();
        for arg in args.iter_mut() {
            arg_tys.push(self.infer_expr(env, arg)?);
        }

        // Check argument count (excluding self)
        let expected_param_count = method_def.params.len();
        if arg_tys.len() != expected_param_count {
            // Get trait name for error message
            let trait_name = self
                .trait_registry
                .get_trait(trait_id)
                .map_or_else(|| Arc::from("?"), |t| Arc::clone(&t.name));
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: expected_param_count,
                    actual: arg_tys.len(),
                },
                span,
            )
            .with_context(format!("in method call `{trait_name}.{method_name}`")));
        }

        // Unify argument types with parameter types
        // For now, we use the parameter types from the trait method definition
        // In a full implementation, we'd substitute Self with the receiver type
        for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(method_def.params.iter()).enumerate() {
            // Substitute Self in param_ty with the receiver type
            let param_ty = substitute_self(param_ty, &receiver_ty);
            if let Err(e) = self.unify(arg_ty, &param_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of method `{}`",
                    i + 1,
                    method_name
                )));
            }
        }

        // Return the substituted return type
        Ok(substitute_self(&method_def.ret, &receiver_ty))
    }
}

/// Substitute `Self` type references with the actual type.
pub(super) fn substitute_self(ty: &Type, self_ty: &Type) -> Type {
    match ty {
        // Check for a Named type called "Self"
        Type::Named(n) if n.name.as_ref() == "Self" && n.args.is_empty() => self_ty.clone(),
        // Recursively substitute in composite types
        Type::Tuple(elems) => {
            Type::Tuple(elems.iter().map(|t| substitute_self(t, self_ty)).collect())
        }
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_self(t, self_ty)))
                .collect(),
        )),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|t| substitute_self(t, self_ty))
                .collect(),
            substitute_self(&f.ret, self_ty),
            f.abilities.clone(),
        ),
        Type::Named(n) => {
            Type::Named(n.map_args(n.args.iter().map(|t| substitute_self(t, self_ty)).collect()))
        }
        // Other types pass through unchanged
        _ => ty.clone(),
    }
}

/// Map binary operators to their corresponding trait and method names.
/// Returns `(trait_name, method_name)` if the operator can be overloaded.
fn operator_trait(op: BinaryOp) -> Option<(&'static str, &'static str)> {
    match op {
        BinaryOp::Add => Some(("Add", "add")),
        BinaryOp::Sub => Some(("Sub", "sub")),
        BinaryOp::Mul => Some(("Mul", "mul")),
        BinaryOp::Div => Some(("Div", "div")),
        BinaryOp::Mod => Some(("Mod", "rem")),
        BinaryOp::Eq | BinaryOp::Ne => Some(("Eq", "eq")),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => Some(("Ord", "cmp")),
        // Logical operators cannot be overloaded
        BinaryOp::And | BinaryOp::Or => None,
    }
}

/// Get the return type for an overloaded operator.
fn operator_return_type(op: BinaryOp, operand_ty: &Type) -> Type {
    match op {
        // Arithmetic operators return the same type
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            operand_ty.clone()
        }
        // Comparison operators return Bool
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            Type::bool()
        }
        // Logical operators (not overloadable, but included for completeness)
        BinaryOp::And | BinaryOp::Or => Type::bool(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, HandlerLiteralMethod, MatchArm, Param, Pattern};
    use crate::infer::Scheme;

    #[test]
    fn test_infer_literal() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::number(42.0);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::number());

        let mut expr = Expr::string("hello");
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::string());

        let mut expr = Expr::bool(true);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::bool());
    }

    #[test]
    fn test_infer_binary_arithmetic() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::number());
    }

    #[test]
    fn test_infer_binary_comparison() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::bool());
    }

    #[test]
    fn test_infer_if_then_else() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr =
            Expr::if_then_else(Expr::bool(true), Expr::number(1.0), Some(Expr::number(2.0)));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::number());
    }

    #[test]
    fn test_infer_if_then_else_mismatch() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::if_then_else(
            Expr::bool(true),
            Expr::number(1.0),
            Some(Expr::string("hello")),
        );
        assert!(infer.infer_expr(&env, &mut expr).is_err());
    }

    #[test]
    fn test_infer_tuple() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::tuple(vec![Expr::number(1.0), Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Tuple(vec![Type::number(), Type::string()]));
    }

    #[test]
    fn test_infer_record() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(
            ty,
            Type::record([("x", Type::number()), ("y", Type::number())])
        );
    }

    #[test]
    fn test_infer_lambda() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // (x) => x + 1
        let mut expr = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        let ty = infer.apply(&ty);
        assert_eq!(ty, Type::function(vec![Type::number()], Type::number()));
    }

    #[test]
    fn test_infer_lambda_call() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // ((x) => x + 1)(42)
        let lambda = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let mut expr = Expr::call(lambda, vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::number());
    }

    #[test]
    fn test_infer_let_polymorphism() {
        let mut infer = Infer::new();
        let mut env = TypeEnv::new();

        // identity: forall a. a -> a
        env.insert(
            0,
            "id".into(),
            Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0))),
        );

        // id(42) should be number
        let mut expr = Expr::call(Expr::local(0), vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::number());

        // id("hello") should be string
        let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::string());
    }

    #[test]
    fn test_generalize() {
        let infer = Infer::new();
        let env = TypeEnv::new();

        // A type with free variable should generalize
        let ty = Type::function(vec![Type::var(0)], Type::var(0));
        let scheme = infer.generalize(&env, &ty);

        assert_eq!(scheme.vars, vec![0]);
    }

    #[test]
    fn test_instantiate() {
        let mut infer = Infer::new();

        let scheme = Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0)));
        let ty = infer.instantiate(&scheme);

        // Should get a fresh type variable, not '0
        if let Type::Function(f) = ty {
            assert!(matches!(f.params[0], Type::Var(_)));
            assert!(matches!(*f.ret, Type::Var(_)));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_resolve_holes_simple() {
        let mut infer = Infer::new();

        // Hole becomes a fresh type variable
        let resolved = infer.resolve_holes(&Type::Hole);
        assert!(matches!(resolved, Type::Var(_)));
    }

    #[test]
    fn test_resolve_holes_nested() {
        let mut infer = Infer::new();

        // Holes in nested types get resolved
        let func = Type::function(vec![Type::Hole], Type::Hole);
        let resolved = infer.resolve_holes(&func);

        if let Type::Function(f) = resolved {
            assert!(matches!(f.params[0], Type::Var(_)));
            assert!(matches!(*f.ret, Type::Var(_)));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_resolve_holes_partial() {
        let mut infer = Infer::new();

        // Mix of concrete types and holes
        let tuple = Type::Tuple(vec![Type::number(), Type::Hole, Type::string()]);
        let resolved = infer.resolve_holes(&tuple);

        if let Type::Tuple(elems) = resolved {
            assert_eq!(elems[0], Type::number());
            assert!(matches!(elems[1], Type::Var(_)));
            assert_eq!(elems[2], Type::string());
        } else {
            panic!("Expected tuple type");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literal type checking tests (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────

    /// An `Infer` with prelude-style test abilities registered:
    /// `Printer.go(message: string): ()` and
    /// `Clock { now(): number; wait(duration: number): (); }`.
    fn infer_with_test_prelude() -> Infer {
        use crate::ability_resolver::{DynAbility, DynMethod};

        let mut infer = Infer::new();
        infer.ability_resolver.register_dynamic_in_namespace(
            "platform",
            DynAbility {
                id: crate::types::AbilityId::from_bytes([7; 32]),
                name: "Printer".into(),
                methods: vec![DynMethod {
                    id: 0,
                    name: "go".into(),
                    param_names: vec![],
                    params: vec![Type::string()],
                    ret: Type::Unit,
                    quantified: vec![],
                }],
                dependencies: vec![],
            },
        );
        infer.ability_resolver.register_dynamic_in_namespace(
            "platform",
            DynAbility {
                id: crate::types::AbilityId::from_bytes([8; 32]),
                name: "Clock".into(),
                methods: vec![
                    DynMethod {
                        id: 0,
                        name: "now".into(),
                        param_names: vec![],
                        params: vec![],
                        ret: Type::number(),
                        quantified: vec![],
                    },
                    DynMethod {
                        id: 1,
                        name: "wait".into(),
                        param_names: vec![],
                        params: vec![Type::number()],
                        ret: Type::Unit,
                        quantified: vec![],
                    },
                ],
                dependencies: vec![],
            },
        );
        infer
    }

    #[test]
    fn test_handler_literal_prelude_ability() {
        let mut infer = infer_with_test_prelude();
        let env = TypeEnv::new();

        // { go(msg) => resume(()) }
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "go",
            vec![Param::new(1, "msg")],
            Expr::unit(), // resume(()) - simplified for test
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Printer>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(
                handler_ty.ability,
                crate::types::AbilityId::from_bytes([7; 32])
            );
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_exception_throw() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { throw(err) => ... }
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "throw",
            vec![Param::new(1, "err")],
            Expr::unit(),
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Exception>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, ambient_core::exception::ability_id());
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_multi_method() {
        let mut infer = infer_with_test_prelude();
        let env = TypeEnv::new();

        // { now() => resume(0.0), wait(duration) => resume(()) }
        let mut expr = Expr::handler_literal(vec![
            HandlerLiteralMethod::new("now", vec![], Expr::number(0.0)),
            HandlerLiteralMethod::new("wait", vec![Param::new(1, "duration")], Expr::unit()),
        ]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Clock>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(
                handler_ty.ability,
                crate::types::AbilityId::from_bytes([8; 32])
            );
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_unknown_method() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { unknown_method(x) => ... } - doesn't match any ability
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "unknown_method",
            vec![Param::new(1, "x")],
            Expr::unit(),
        )]);

        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_err(),
            "Should fail when methods don't match any ability"
        );
    }

    #[test]
    fn test_handler_literal_wrong_arity() {
        let mut infer = infer_with_test_prelude();
        let env = TypeEnv::new();

        // { go(a, b) => ... } - Printer.go takes 1 arg, not 2
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "go",
            vec![Param::new(1, "a"), Param::new(2, "b")],
            Expr::unit(),
        )]);

        let result = infer.infer_expr(&env, &mut expr);
        assert!(result.is_err(), "Should fail when arity doesn't match");

        // Check error message mentions arity
        if let Err(err) = result {
            let msg = format!("{}", err.kind);
            assert!(
                msg.contains("expects 1 parameters") || msg.contains("expected 1"),
                "Error should mention expected arity: {}",
                msg
            );
        }
    }

    #[test]
    fn test_handler_literal_partial_handler() {
        let mut infer = infer_with_test_prelude();
        let env = TypeEnv::new();

        // { now() => ... } - only handles now, not wait
        // This should be allowed (partial handlers can be composed)
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "now",
            vec![],
            Expr::number(0.0),
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert!(
            matches!(ty, Type::Handler(_)),
            "Partial handlers should be allowed"
        );
    }

    #[test]
    fn test_handler_literal_params_take_declared_types() {
        let mut infer = infer_with_test_prelude();
        let env = TypeEnv::new();

        // { go(msg) => msg + "!" } — Printer.go(message: string), so msg is
        // a string and string concatenation type-checks.
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "go",
            vec![Param::new(1, "msg")],
            Expr::binary(BinaryOp::Add, Expr::local(1), Expr::string("!")),
        )]);
        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_ok(),
            "handler param should have its declared type in scope: {result:?}"
        );

        // { go(msg) => msg + 1 } — msg is a string, not a number: rejected.
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "go",
            vec![Param::new(1, "msg")],
            Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
        )]);
        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_err(),
            "handler param must be constrained to the declared param type"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Error case coverage tests (CQ-012)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_error_undefined_variable() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Reference to a variable that doesn't exist
        let mut expr = Expr::variable("undefined_var");
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::UndefinedVariable { .. }),
                "Expected UndefinedVariable, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_field_not_found() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Access a field that doesn't exist on a record
        let record = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let mut expr = Expr::field_access(record, "z");
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::FieldNotFound { .. }),
                "Expected FieldNotFound, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_tuple_index_out_of_bounds() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Access index 5 on a 2-element tuple
        let tuple = Expr::tuple(vec![Expr::number(1.0), Expr::number(2.0)]);
        let mut expr = Expr::tuple_index(tuple, 5);
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::TupleIndexOutOfBounds { .. }),
                "Expected TupleIndexOutOfBounds, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_calling_non_function() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Try to call a number as a function - type inference will produce TypeMismatch
        // because it tries to unify Number with Function type
        let mut expr = Expr::call(Expr::number(42.0), vec![Expr::number(1.0)]);
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            // The error is TypeMismatch because unification fails when trying
            // to match Number with a function type
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch when calling non-function, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_non_boolean_if_condition() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // if with a number condition instead of bool - produces TypeMismatch
        // when unifying condition type (Number) with Bool
        let mut expr = Expr::if_then_else(
            Expr::number(1.0),
            Expr::number(2.0),
            Some(Expr::number(3.0)),
        );
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            // Unification error: expected Number (condition type), actual Bool (target type)
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch for non-bool condition, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_match_arms_different_types() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Match with arms returning different types - produces TypeMismatch
        // when unifying first arm type with subsequent arm types
        let mut expr = Expr::match_expr(
            Expr::number(1.0),
            vec![
                MatchArm::new(Pattern::wildcard(), Expr::number(1.0)),
                MatchArm::new(Pattern::wildcard(), Expr::string("hello")),
            ],
        );
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(
                    &err.kind,
                    TypeErrorKind::TypeMismatch { expected, actual }
                        if expected.as_primitive() == Some(crate::types::Primitive::Number)
                            && actual.as_primitive() == Some(crate::types::Primitive::String)
                ),
                "Expected TypeMismatch between Number and String, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_wrong_argument_count() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Call a function with wrong number of arguments - produces TypeMismatch
        // because function types don't match
        let lambda = Expr::lambda(
            vec![Param::new(0, "x"), Param::new(1, "y")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
        );
        let mut expr = Expr::call(lambda, vec![Expr::number(1.0)]); // Only 1 arg, needs 2
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        // This produces a TypeMismatch because the inferred function type
        // doesn't match the application
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch for wrong argument count, got {:?}",
                err.kind
            );
        }
    }
}
