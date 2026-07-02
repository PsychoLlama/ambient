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
use super::{type_error, Infer, InferResult, TypeEnv, TypeError, TypeErrorKind};
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
        let span = (expr.span.start, expr.span.end);
        let ty = match &mut expr.kind {
            ExprKind::Unit => Type::Unit,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Number(_) => Type::Number,
            ExprKind::String(_) => Type::String,

            ExprKind::Local(id) => {
                let scheme = env
                    .get(*id)
                    .ok_or_else(|| type_error(TypeErrorKind::UndefinedBinding { id: *id }, span))?;
                self.instantiate(scheme)
            }

            ExprKind::Name(name) => {
                let scheme = env.get_by_name(&name.name).ok_or_else(|| {
                    type_error(
                        TypeErrorKind::UndefinedVariable {
                            name: name.name.clone(),
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
                // Look up the type alias
                let type_alias = self.get_type_alias(&type_name.name).ok_or_else(|| {
                    type_error(
                        TypeErrorKind::UndefinedTypeName {
                            name: type_name.name.clone(),
                        },
                        span,
                    )
                })?;
                let full_type = type_alias.clone();

                // Get the expected record type (unwrap Nominal if present)
                let expected_record = match &full_type {
                    Type::Nominal(nom) => match nom.inner.as_ref() {
                        Type::Record(r) => r.clone(),
                        other => {
                            return Err(type_error(
                                TypeErrorKind::NotARecordType { ty: other.clone() },
                                span,
                            ))
                        }
                    },
                    Type::Record(r) => r.clone(),
                    other => {
                        return Err(type_error(
                            TypeErrorKind::NotARecordType { ty: other.clone() },
                            span,
                        ))
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
                        self.unify(&operand_ty, &Type::Number, span)?;
                        Type::Number
                    }
                    UnaryOp::Not => {
                        self.unify(&operand_ty, &Type::Bool, span)?;
                        Type::Bool
                    }
                }
            }

            ExprKind::If(cond, then_branch, else_branch) => {
                let cond_ty = self.infer_expr(env, cond)?;
                self.unify(&cond_ty, &Type::Bool, span)?;

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
                for arm in arms {
                    let arm_env = self.infer_pattern(env, &arm.pattern, &scrutinee_ty)?;
                    // Clone the arm body to avoid mutable borrow issues
                    let mut body = arm.body.clone();
                    let arm_ty = self.infer_expr(&arm_env, &mut body)?;
                    self.unify(&result_ty, &arm_ty, span)?;
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
                let saved = std::mem::replace(&mut self.current_abilities, AbilitySet::Empty);
                let body_result = self.infer_expr(&lambda_env, &mut lambda.body);
                let lambda_abilities = std::mem::replace(&mut self.current_abilities, saved);
                let ret_ty = body_result?;

                Type::function_with_abilities(
                    param_tys,
                    ret_ty,
                    self.apply_abilities(&lambda_abilities),
                )
            }

            ExprKind::Call(callee, args) => {
                // Check for intrinsic functions first
                if let ExprKind::Name(name) = &callee.kind {
                    if let Some(ret_ty) = self.try_infer_intrinsic(env, name, args, span)? {
                        return Ok(ret_ty);
                    }
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

            ExprKind::Suspend(ability_call) => self.infer_suspend(env, ability_call, span)?,

            ExprKind::Handle(handle_expr) => self.infer_handle(env, handle_expr, span)?,

            ExprKind::Resume(value) => {
                // Resume transfers control to a continuation.
                // The value type should match what the continuation expects.
                // For now, just type-check the value and return a fresh type variable,
                // since resume doesn't return normally.
                let _value_ty = self.infer_expr(env, value)?;

                // Resume doesn't return normally, so we use a fresh type variable.
                // In a more complete implementation, this would be a never type (!).
                self.fresh()
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

        // Get the ability's method signatures
        let ability_signatures = self.get_ability_method_signatures(ability_id);

        // Verify each method in the handler matches the ability
        for method in &mut handler_lit.methods {
            // Find the corresponding ability method signature
            let sig = ability_signatures
                .iter()
                .find(|(name, _, _)| name == method.method.as_ref());

            if let Some((_, expected_param_count, expected_return_ty)) = sig {
                // Check arity (excluding implicit continuation parameter)
                if method.params.len() != *expected_param_count {
                    return Err(type_error(
                        TypeErrorKind::HandlerMethodArityMismatch {
                            ability: ability_name.clone(),
                            method: method.method.clone(),
                            expected: *expected_param_count,
                            actual: method.params.len(),
                        },
                        (method.span.start, method.span.end),
                    ));
                }

                // Type-check the handler body with method parameters in scope
                let mut method_env = env.extend();
                for param in &method.params {
                    // Parameter types are inferred (fresh type variables)
                    let param_ty = param.ty.clone().unwrap_or_else(|| self.fresh());
                    method_env.insert_mono(param.id, param.name.clone(), param_ty);
                }

                // The handler body type should be compatible with resume behavior
                // For most methods, the body should eventually call resume()
                // The resume value type determines what's returned to the call site
                let body_ty = self.infer_expr(&method_env, &mut method.body)?;

                // If we have a concrete expected return type, try to unify
                // (Hole means polymorphic - we don't constrain)
                if *expected_return_ty != Type::Hole && *expected_return_ty != Type::Never {
                    // Note: The body type isn't directly the return type -
                    // the return type is what resume() is called with.
                    // For now, we just type-check the body without strict constraints.
                    let _ = body_ty; // Body is checked but not constrained
                }
            } else {
                // Method not found in ability
                return Err(type_error(
                    TypeErrorKind::HandlerUnknownMethod {
                        ability: ability_name.clone(),
                        method: method.method.clone(),
                    },
                    (method.span.start, method.span.end),
                ));
            }
        }

        // Handlers don't need to provide all methods - partial handlers are allowed
        // (they can be composed with other handlers that provide missing methods)

        Ok(Type::handler(ability_id))
    }

    /// Infer the type of a sandbox expression.
    fn infer_sandbox(
        &mut self,
        env: &TypeEnv,
        sandbox_expr: &mut crate::ast::SandboxExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Sandbox type checking (Milestone 14)
        //
        // A sandbox restricts the abilities available within its body to only
        // those explicitly allowed. This enables running untrusted code with
        // limited capabilities.
        //
        // 1. Save the current ability context
        // 2. Create a new ability context with only allowed abilities
        // 3. Type-check the body in this restricted context
        // 4. Verify the body only uses allowed abilities
        // 5. Restore the original ability context

        // Save current abilities
        let saved_abilities = self.current_abilities.clone();

        // Reset to empty abilities - only allowed abilities will be available
        self.reset_abilities();

        // Convert allowed ability names to IDs
        let allowed_ability_ids: Vec<_> = sandbox_expr
            .allowed_abilities
            .iter()
            .filter_map(|name| self.ability_name_to_id(&name.name))
            .collect();

        // Check for unknown abilities
        for ability_name in &sandbox_expr.allowed_abilities {
            if self.ability_name_to_id(&ability_name.name).is_none() {
                return Err(type_error(
                    TypeErrorKind::UnknownAbility {
                        name: ability_name.name.clone(),
                    },
                    span,
                ));
            }
        }

        // Infer the body expression
        let body_ty = self.infer_expr(env, &mut sandbox_expr.body.clone())?;

        // Get the abilities required by the body
        let body_abilities = self.current_abilities.clone();

        // Verify that the body only uses allowed abilities
        if let AbilitySet::Concrete(required_abilities) = &body_abilities {
            // Check each required ability is in the allowed list
            for ability_id in required_abilities {
                if !allowed_ability_ids.contains(ability_id) {
                    let ability_name = self.ability_id_to_name(*ability_id).unwrap_or("unknown");
                    return Err(type_error(
                        TypeErrorKind::SandboxAbilityViolation {
                            ability: ability_name.into(),
                            allowed: sandbox_expr
                                .allowed_abilities
                                .iter()
                                .map(|n| n.name.clone())
                                .collect(),
                        },
                        span,
                    ));
                }
            }
        }
        // Note: AbilitySet::Empty (pure), Var, and Row are allowed
        // Polymorphic abilities can't be statically checked

        // Restore saved abilities (sandbox doesn't add any abilities to the outer context)
        self.current_abilities = saved_abilities;

        Ok(body_ty)
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
                if op == BinaryOp::Add && left_ty == Type::String {
                    self.unify(&right_ty, &Type::String, span)?;
                    return Ok(Type::String);
                }
                self.unify(&left_ty, &Type::Number, span)?;
                self.unify(&right_ty, &Type::Number, span)?;
                Ok(Type::Number)
            }

            // Comparison operators: a -> a -> Bool
            BinaryOp::Eq | BinaryOp::Ne => {
                self.unify(&left_ty, &right_ty, span)?;
                Ok(Type::Bool)
            }

            // Ordering operators: Number -> Number -> Bool
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                self.unify(&left_ty, &Type::Number, span)?;
                self.unify(&right_ty, &Type::Number, span)?;
                Ok(Type::Bool)
            }

            // Logical operators: Bool -> Bool -> Bool
            BinaryOp::And | BinaryOp::Or => {
                self.unify(&left_ty, &Type::Bool, span)?;
                self.unify(&right_ty, &Type::Bool, span)?;
                Ok(Type::Bool)
            }
        }
    }

    /// Infer the type of a method call expression.
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
        let Some((trait_id, method_def, method_symbol)) =
            self.trait_registry.find_method(nominal.uuid, method_name)
        else {
            return Err(type_error(
                TypeErrorKind::MethodNotFound {
                    method: Arc::clone(method_name),
                    ty: receiver_ty.clone(),
                },
                span,
            ));
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
        Type::Named(n) => Type::Named(crate::types::NamedType::new(
            Arc::clone(&n.name),
            n.args.iter().map(|t| substitute_self(t, self_ty)).collect(),
        )),
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
            Type::Bool
        }
        // Logical operators (not overloadable, but included for completeness)
        BinaryOp::And | BinaryOp::Or => Type::Bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, HandlerLiteralMethod, MatchArm, Param, Pattern};
    use crate::infer::Scheme;
    use crate::types::TypeVar;

    #[test]
    fn test_infer_literal() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::number(42.0);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);

        let mut expr = Expr::string("hello");
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);

        let mut expr = Expr::bool(true);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_binary_arithmetic() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_binary_comparison() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_if_then_else() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr =
            Expr::if_then_else(Expr::bool(true), Expr::number(1.0), Some(Expr::number(2.0)));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
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
        assert_eq!(ty, Type::Tuple(vec![Type::Number, Type::String]));
    }

    #[test]
    fn test_infer_record() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::record([("x", Type::Number), ("y", Type::Number)]));
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
        assert_eq!(ty, Type::function(vec![Type::Number], Type::Number));
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
        assert_eq!(ty, Type::Number);
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
        assert_eq!(ty, Type::Number);

        // id("hello") should be string
        let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);
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
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
            assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_resolve_holes_simple() {
        let mut infer = Infer::new();

        // Hole becomes a fresh type variable
        let resolved = infer.resolve_holes(&Type::Hole);
        assert!(matches!(resolved, Type::Var(TypeVar::Unbound(_))));
    }

    #[test]
    fn test_resolve_holes_nested() {
        let mut infer = Infer::new();

        // Holes in nested types get resolved
        let func = Type::function(vec![Type::Hole], Type::Hole);
        let resolved = infer.resolve_holes(&func);

        if let Type::Function(f) = resolved {
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
            assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_resolve_holes_partial() {
        let mut infer = Infer::new();

        // Mix of concrete types and holes
        let tuple = Type::Tuple(vec![Type::Number, Type::Hole, Type::String]);
        let resolved = infer.resolve_holes(&tuple);

        if let Type::Tuple(elems) = resolved {
            assert_eq!(elems[0], Type::Number);
            assert!(matches!(elems[1], Type::Var(TypeVar::Unbound(_))));
            assert_eq!(elems[2], Type::String);
        } else {
            panic!("Expected tuple type");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literal type checking tests (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_handler_literal_console_print() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => resume(()) }
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::unit(), // resume(()) - simplified for test
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Console>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, 0x0001); // Console ability ID
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
            assert_eq!(handler_ty.ability, 0x0002); // Exception ability ID
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_time_methods() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { now() => resume(0.0), wait(duration) => resume(()) }
        let mut expr = Expr::handler_literal(vec![
            HandlerLiteralMethod::new("now", vec![], Expr::number(0.0)),
            HandlerLiteralMethod::new("wait", vec![Param::new(1, "duration")], Expr::unit()),
        ]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Time>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, 0x0003); // Time ability ID
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
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(a, b) => ... } - Console.print takes 1 arg, not 2
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
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
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => ... } - only handles print, not println/eprint
        // This should be allowed (partial handlers can be composed)
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::unit(),
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert!(
            matches!(ty, Type::Handler(_)),
            "Partial handlers should be allowed"
        );
    }

    #[test]
    fn test_handler_literal_method_body_type_checked() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => msg + 1 } - body uses msg (should type-check)
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
        )]);

        // This should succeed - the parameter 'msg' is in scope
        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_ok(),
            "Handler method body should type-check with params in scope"
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
                    err.kind,
                    TypeErrorKind::TypeMismatch {
                        expected: Type::Number,
                        actual: Type::String
                    }
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
