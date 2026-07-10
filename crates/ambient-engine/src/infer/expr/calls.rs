//! Inference for method dispatch: inherent methods, associated
//! (`Type::method(...)`) calls, and trait method calls — plus the shared
//! `Self`-substitution helper.

use std::sync::Arc;

use crate::ast::Expr;
use crate::infer::error::BoxedTypeErrorExt;
use crate::infer::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::types::Type;

impl Infer {
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
    /// to `core::option::map`. Argument type errors surface as `Err`.
    pub(super) fn try_infer_associated_call(
        &mut self,
        env: &TypeEnv,
        type_name: &str,
        method_name: &str,
        args: &mut [Expr],
        span: (u32, u32),
    ) -> InferResult<Option<(Arc<str>, Type)>> {
        use crate::infer::inherent::ImplKey;

        // Resolve the leading segment to an impl-target key: a nominal type
        // alias or opaque generic head in scope (a container like `List`
        // arrives through the prelude), or an enum.
        let key = if let Some(uuid) = self
            .get_type_alias(type_name)
            .and_then(crate::infer::AliasTarget::impl_uuid)
        {
            Some(ImplKey::Nominal(uuid))
        } else {
            // A declared enum keys on its uuid (matching its receiver-form
            // dispatch); prelude enums key on their reserved head name.
            self.enum_registry
                .get(type_name)
                .map(|info| match info.uuid {
                    Some(uuid) => ImplKey::Nominal(uuid),
                    None => ImplKey::Named(type_name.into()),
                })
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
        let Some(Type::Nominal(nominal)) = self
            .get_type_alias(type_name)
            .and_then(crate::infer::AliasTarget::whole)
            .cloned()
        else {
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
    pub(super) fn infer_method_call(
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
        let (trait_uuid, method_def, method_symbol) =
            match self.trait_registry.find_method(nominal.uuid, method_name) {
                crate::types::MethodLookup::Found {
                    trait_uuid,
                    method,
                    symbol,
                } => (trait_uuid, method, symbol),
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
                .get_trait(trait_uuid)
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
pub(in crate::infer) fn substitute_self(ty: &Type, self_ty: &Type) -> Type {
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
