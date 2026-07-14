//! Inference for method dispatch: inherent methods, associated
//! (`Type::method(...)`) calls, and trait method calls — plus the shared
//! `Self`-substitution helper.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Dicts, Expr, ResolvedMethod};
use crate::infer::check::{resolve_declared_with, substitute_type_params};
use crate::infer::error::BoxedTypeErrorExt;
use crate::infer::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::types::{AbilitySet, AbilityVarId, TraitMethodDef, Type, TypeVarId};

impl Infer {
    /// Type-check a call to an inherent method against its instantiated
    /// scheme. `receiver_ty` is `Some` for dot calls (unified with parameter
    /// 0, which binds the impl's type parameters) and `None` for associated
    /// `Type::method(...)` calls. A bounded scheme (`impl<T: Eq> List<T>`)
    /// records its dictionary constraints against `dicts`.
    /// Returns `(return_type, callee_fn_type)`: the call's result plus the
    /// method's fully-instantiated function type. The latter is what tooling
    /// (hover) shows for the callee of a `Type::method(...)` associated call,
    /// whose name node the checker rewrites to a bare dispatch symbol and never
    /// infers; instance-call callers ignore it.
    #[allow(clippy::too_many_arguments)]
    fn infer_inherent_call(
        &mut self,
        env: &TypeEnv,
        method: &crate::infer::inherent::InherentMethod,
        receiver_ty: Option<&Type>,
        args: &mut [Expr],
        span: (u32, u32),
        resolved_method: &mut Option<ResolvedMethod>,
        dicts: &mut Option<Dicts>,
    ) -> InferResult<(Type, Type)> {
        let fn_ty = self.instantiate_bounded(&method.scheme, span, dicts);
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
            // Push the parameter's (instantiated) type into the argument as
            // its expected type: an unannotated lambda argument seeds its
            // parameter types from it (bidirectional checking). See
            // `infer_expr_expecting`.
            let arg_ty = self.infer_expr_expecting(env, arg, Some(param_ty))?;
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

        *resolved_method = Some(ResolvedMethod::Symbol(Arc::clone(&method.symbol)));
        let ret = self.apply(&f.ret);
        let fn_ty = Type::function_with_abilities(
            f.params.iter().map(|p| self.apply(p)).collect(),
            ret.clone(),
            abilities,
        );
        Ok((ret, fn_ty))
    }

    /// Try to resolve a `Type::method(args)` associated-function call.
    ///
    /// Returns `Some((symbol, return_type, callee_fn_type))` when `type_name`
    /// names a type with a no-`self` method: an inherent associated method
    /// (checked first), or a trait associated method such as `Default::default`
    /// (nominal types only). `callee_fn_type` is the method's instantiated
    /// function type — the caller records it on the (rewritten) callee node so
    /// hover shows a real signature, not the bare dispatch symbol. Returns
    /// `None` when this is not such a call — the caller then falls back to
    /// ordinary qualified name resolution, so module companion functions like
    /// `Option::map(opt, f)` keep resolving to `core::option::map`. Argument
    /// type errors surface as `Err`.
    pub(super) fn try_infer_associated_call(
        &mut self,
        env: &TypeEnv,
        type_name: &str,
        method_name: &str,
        args: &mut [Expr],
        span: (u32, u32),
        dicts: &mut Option<Dicts>,
    ) -> InferResult<Option<(Arc<str>, Type, Type)>> {
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
            let (ret, fn_ty) =
                self.infer_inherent_call(env, &method, None, args, span, &mut resolved, dicts)?;
            return Ok(resolved
                .and_then(|r| r.as_symbol().cloned())
                .map(|s| (s, ret, fn_ty)));
        }

        // The leading segment must name a nominal type that can carry a trait
        // impl — a struct, primitive, `extern`, or a declared/prelude enum,
        // all keyed by a uuid. Build its `Self` type with fresh type variables
        // so the call's arguments (and any annotation) pin them, then derive
        // the identity the impl keys on. This is the same "what can be an impl
        // target" question the instance path answers (`impl_key_for`), so enum
        // associated functions (`Shape::default()`) dispatch exactly like
        // their instance-method cousins do since enums became impl targets.
        let Some(self_ty) = self.associated_self_type(type_name) else {
            return Ok(None);
        };
        let Some(type_uuid) = crate::infer::inherent::impl_key_for(&self_ty).and_then(|k| k.uuid())
        else {
            return Ok(None);
        };

        // The method must exist and be associated (no `self`); an instance
        // method reached this way is not a valid associated call.
        let (trait_uuid, method_def, symbol) =
            match self.trait_registry.find_method(type_uuid, method_name) {
                crate::types::MethodLookup::Found {
                    trait_uuid,
                    method,
                    symbol,
                } if !method.has_self => (trait_uuid, method.clone(), symbol),
                _ => return Ok(None),
            };

        // A conditional impl's own bounds and the method's own bounds both
        // thread as trailing dictionaries, exactly like an instance call; a
        // bound-less applied impl has its instantiation coverage checked.
        let generic_impl = self
            .trait_registry
            .get_impl(trait_uuid, type_uuid)
            .filter(|imp| imp.is_generic)
            .map(|imp| (imp.target.clone(), imp.bounds.clone()));

        let assoc_trait_name = self
            .trait_registry
            .get_trait(trait_uuid)
            .map_or_else(|| Arc::from("?"), |t| Arc::clone(&t.name));
        let (params, ret, abilities, type_var_map) =
            self.instantiate_trait_method_mapped(&method_def, &self_ty);
        let method_dicts = self.resolve_method_bound_dicts(&method_def, &type_var_map);
        *dicts = self.record_trait_dispatch_dicts(
            generic_impl,
            &self_ty,
            method_dicts,
            &assoc_trait_name,
            span,
        );

        if args.len() != params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: params.len(),
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in associated call `{type_name}::{method_name}`")));
        }

        for (i, (arg, param_ty)) in args.iter_mut().zip(params.iter()).enumerate() {
            // Seed an unannotated lambda argument from the parameter's
            // instantiated type (bidirectional checking).
            let arg_ty = self.infer_expr_expecting(env, arg, Some(param_ty))?;
            if let Err(e) = self.unify(&arg_ty, param_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of associated call `{type_name}::{method_name}`",
                    i + 1
                )));
            }
        }

        let abilities = self.apply_abilities(&abilities);
        self.require_abilities(&abilities);

        let ret = self.apply(&ret);
        let fn_ty = Type::function_with_abilities(
            params.iter().map(|p| self.apply(p)).collect(),
            ret.clone(),
            abilities,
        );
        Ok(Some((symbol, ret, fn_ty)))
    }

    /// Build the `Self` type for a `Type::method(...)` associated call: the
    /// named nominal type with its parameters instantiated to fresh inference
    /// variables, so the call's arguments (and any surrounding annotation) pin
    /// them. A non-generic type's body has no parameters and comes through
    /// unchanged. Returns `None` when the name is not a nominal type (a
    /// structural `type` alias, or an unknown name), so the caller falls back
    /// to ordinary qualified-name resolution.
    fn associated_self_type(&mut self, type_name: &str) -> Option<Type> {
        use crate::infer::AliasTarget;

        // A declared or prelude enum: its head plus one fresh argument per
        // type parameter. A generic enum's associated function (`Wrap::
        // make()`) instantiates just like a struct's.
        if let Some(info) = self.enum_registry.get(type_name).cloned() {
            let args = info.type_params.iter().map(|_| self.fresh()).collect();
            return Some(Type::Named(crate::types::NamedType::with_identity(
                Arc::clone(&info.name),
                args,
                info.uuid,
            )));
        }

        match self.get_type_alias(type_name)?.clone() {
            // A non-generic struct or primitive: the body is already the
            // concrete nominal, no parameters to instantiate.
            AliasTarget::Whole(ty) => Some(ty),
            // A fielded generic struct: substitute fresh variables for its
            // parameters into the record body, yielding the `Type::Nominal`
            // the instance path would see — so a conditional impl's dictionary
            // solves against the recovered assignments once arguments land.
            AliasTarget::GenericStruct { type_params, body } => {
                let map: HashMap<Arc<str>, Type> = type_params
                    .into_iter()
                    .map(|param| (param, self.fresh()))
                    .collect();
                Some(crate::infer::check::substitute_named(&body, &map))
            }
            // An opaque generic head (an `extern` container): the applied
            // `Named(head, args, uuid)` shape, args fresh.
            AliasTarget::OpaqueGeneric { uuid, arity } => {
                let args = (0..arity).map(|_| self.fresh()).collect();
                Some(Type::Named(crate::types::NamedType::with_identity(
                    Arc::from(type_name),
                    args,
                    Some(uuid),
                )))
            }
        }
    }

    /// Infer the type of a method call expression.
    ///
    /// Resolution order: inherent methods first (any type with an impl-key
    /// identity — nominal, enum, built-in container, primitive), then
    /// bound methods (rigid type-parameter receivers dispatch through the
    /// enclosing function's dictionary), then trait methods (nominal
    /// receivers only). Inherent methods shadow same-named trait methods,
    /// so adding an inherent method is a deliberate, local override —
    /// never silent ambiguity.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn infer_method_call(
        &mut self,
        env: &TypeEnv,
        receiver: &mut Expr,
        method_name: &Arc<str>,
        method_span: crate::ast::Span,
        args: &mut [Expr],
        resolved_method: &mut Option<ResolvedMethod>,
        dicts: &mut Option<Dicts>,
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
            return self
                .infer_inherent_call(
                    env,
                    &method,
                    Some(&receiver_ty),
                    args,
                    span,
                    resolved_method,
                    dicts,
                )
                .map(|(ret, _)| ret);
        }

        // A rigid type parameter dispatches through its bounds: `x.eq(y)`
        // where `x: T` and `T: Eq` compiles as a slot access into the
        // enclosing function's Eq dictionary for T.
        if let Type::Param(param) = &receiver_ty {
            return self.infer_bound_method_call(
                env,
                Arc::clone(param),
                &receiver_ty,
                method_name,
                args,
                span,
                resolved_method,
                dicts,
            );
        }

        // Trait methods dispatch on the receiver's nominal identity — a
        // struct/primitive/`extern` nominal, or a declared/prelude enum, all
        // of which carry a UUID. Structural receivers (records, tuples,
        // functions) have no identity and no methods.
        let Some(type_uuid) =
            crate::infer::inherent::impl_key_for(&receiver_ty).and_then(|k| k.uuid())
        else {
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
            match self.trait_registry.find_method(type_uuid, method_name) {
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
        *resolved_method = Some(ResolvedMethod::Symbol(method_symbol));

        // A conditional (generic) impl's method carries hidden trailing
        // dictionaries for the impl's own bounds; a method-level-bounded trait
        // method (`fn tag<U: Eq>`) carries dictionaries for *its* bounds. Both
        // thread through this direct call site as trailing arguments, in the
        // combined order the compiled impl method allocates them
        // (`alloc_dict_locals(impl ++ method)`).
        let generic_impl = self
            .trait_registry
            .get_impl(trait_uuid, type_uuid)
            .filter(|imp| imp.is_generic)
            .map(|imp| (imp.target.clone(), imp.bounds.clone()));

        // Instantiate the method's generics fresh for this call site: `Self` →
        // the receiver, each method-level type parameter → a fresh inference
        // variable, each `E!` → a fresh ability (row) variable. An effectful
        // argument's row unifies into that variable and then propagates to the
        // caller via `require_abilities` below. The type-var map lets us record
        // the method's own bound dictionaries against those fresh variables.
        // The trait's name, for coverage/arity diagnostics.
        let trait_name = self
            .trait_registry
            .get_trait(trait_uuid)
            .map_or_else(|| Arc::from("?"), |t| Arc::clone(&t.name));

        let (params, ret, abilities, type_var_map) =
            self.instantiate_trait_method_mapped(&method_def, &receiver_ty);
        let method_dicts = self.resolve_method_bound_dicts(&method_def, &type_var_map);
        *dicts = self.record_trait_dispatch_dicts(
            generic_impl,
            &receiver_ty,
            method_dicts,
            &trait_name,
            span,
        );

        // Check argument count (excluding self) before inferring the
        // arguments, so a lambda argument can be seeded from its parameter's
        // instantiated type.
        if args.len() != params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: params.len(),
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in method call `{trait_name}.{method_name}`")));
        }

        // Infer each argument under its instantiated parameter type as the
        // expected type (bidirectional checking: an unannotated lambda seeds
        // its parameter types from it), then unify.
        for (i, (arg, param_ty)) in args.iter_mut().zip(params.iter()).enumerate() {
            let arg_ty = self.infer_expr_expecting(env, arg, Some(param_ty))?;
            if let Err(e) = self.unify(&arg_ty, param_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of method `{}`",
                    i + 1,
                    method_name
                )));
            }
        }

        // The method's declared effect row (with its instantiated variable) is
        // the caller's obligation, exactly as an ordinary call absorbs its
        // callee's `with` row.
        let abilities = self.apply_abilities(&abilities);
        self.require_abilities(&abilities);

        Ok(self.apply(&ret))
    }

    /// Instantiate a trait method's stored (un-instantiated) signature for one
    /// use, substituting `Self` with `self_ty`, each method-level type
    /// parameter with a fresh inference variable, and each `E!` with a fresh
    /// ability (row) variable. Returns the non-self parameter types, the return
    /// type, the declared effect row, and the map from the method's
    /// type-parameter names to the fresh inference variables they were
    /// instantiated to — a call site needs the map to record the method's own
    /// trait-bound dictionaries against those variables (see
    /// `record_trait_dispatch_dicts`). Mirrors how a generic function's scheme
    /// is instantiated at a call site.
    pub(in crate::infer) fn instantiate_trait_method_mapped(
        &mut self,
        method: &TraitMethodDef,
        self_ty: &Type,
    ) -> (Vec<Type>, Type, AbilitySet, HashMap<Arc<str>, TypeVarId>) {
        let type_var_map: HashMap<Arc<str>, TypeVarId> = method
            .type_param_names
            .iter()
            .map(|n| (Arc::clone(n), self.r#gen.fresh_id()))
            .collect();
        let ability_var_map: HashMap<Arc<str>, AbilityVarId> = method
            .ability_var_names
            .iter()
            .map(|n| (Arc::clone(n), self.r#gen.fresh_ability_id()))
            .collect();

        let (params, ret, abilities) =
            self.with_ability_var_scope(ability_var_map.clone(), false, |infer| {
                let instantiate = |infer: &mut Self, ty: &Type| {
                    let ty = substitute_type_params(ty, &type_var_map);
                    let ty = infer.resolve_holes(&ty);
                    substitute_self(&ty, self_ty)
                };
                let params = method
                    .params
                    .iter()
                    .map(|p| instantiate(infer, p))
                    .collect();
                let ret = instantiate(infer, &method.ret);
                let abilities =
                    resolve_declared_with(infer, &method.abilities, &ability_var_map, &method.name);
                (params, ret, abilities)
            });
        (params, ret, abilities, type_var_map)
    }

    /// Resolve a trait method's own bounds (`fn tag<U: Eq>`) into the pending
    /// dictionary constraints a concrete-receiver call must supply: one
    /// `(fresh instantiation variable, resolved bound)` per bound in
    /// `dict_params` order. The variable is the method type parameter's fresh
    /// instantiation, so once argument unification pins it the constraint
    /// solves to the argument type's impl (or a forwarded enclosing dictionary).
    /// An unresolvable bound name is skipped — the same trait bound on the
    /// implementing method reports it, and an error-carrying module never
    /// compiles.
    fn resolve_method_bound_dicts(
        &self,
        method: &TraitMethodDef,
        type_var_map: &HashMap<Arc<str>, TypeVarId>,
    ) -> Vec<(Type, crate::types::TraitBound)> {
        let mut out = Vec::new();
        for (param, bound) in &method.method_bounds {
            let (Some(trait_uuid), Some(&var)) =
                (self.trait_uuid_of(bound), type_var_map.get(param))
            else {
                continue;
            };
            out.push((
                Type::var(var),
                crate::types::TraitBound {
                    trait_uuid,
                    name: Arc::clone(&bound.name),
                },
            ));
        }
        out
    }

    /// Type a method call whose receiver is a rigid type parameter: the
    /// method must come from one of the parameter's declared bounds, and
    /// dispatch is a dictionary-slot access on the enclosing function's
    /// hidden dictionary parameter.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn infer_bound_method_call(
        &mut self,
        env: &TypeEnv,
        param: Arc<str>,
        receiver_ty: &Type,
        method_name: &Arc<str>,
        args: &mut [Expr],
        span: (u32, u32),
        resolved_method: &mut Option<ResolvedMethod>,
        dicts: &mut Option<Dicts>,
    ) -> InferResult<Type> {
        // Find the (unique) bound of this parameter that provides the
        // method. Two bounds providing the same name is ambiguity, exactly
        // like two trait impls.
        let mut found: Option<(usize, crate::types::TraitBound)> = None;
        let mut candidates: Vec<Arc<str>> = Vec::new();
        for (dict_index, (name, bound)) in self.current_bound_params.iter().enumerate() {
            if name != &param {
                continue;
            }
            let provides = self
                .trait_registry
                .get_trait(bound.trait_uuid)
                .is_some_and(|def| {
                    def.methods
                        .iter()
                        .any(|m| m.name.as_ref() == method_name.as_ref() && m.has_self)
                });
            if provides {
                candidates.push(Arc::clone(&bound.name));
                if found.is_none() {
                    found = Some((dict_index, bound.clone()));
                }
            }
        }
        if candidates.len() > 1 {
            return Err(type_error(
                TypeErrorKind::AmbiguousMethod {
                    method: Arc::clone(method_name),
                    ty: receiver_ty.clone(),
                    candidates,
                },
                span,
            ));
        }
        let Some((dict_index, bound)) = found else {
            let bounds: Vec<Arc<str>> = self
                .current_bound_params
                .iter()
                .filter(|(name, _)| name == &param)
                .map(|(_, b)| Arc::clone(&b.name))
                .collect();
            return Err(type_error(
                TypeErrorKind::MethodNotInBounds {
                    method: Arc::clone(method_name),
                    param,
                    bounds,
                },
                span,
            ));
        };

        // The borrow on the registry ends here; clone what we need.
        let trait_def = self
            .trait_registry
            .get_trait(bound.trait_uuid)
            .cloned()
            .ok_or_else(|| {
                type_error(
                    TypeErrorKind::UnknownTrait {
                        name: Arc::clone(&bound.name),
                    },
                    span,
                )
            })?;
        let method_def = trait_def
            .methods
            .iter()
            .find(|m| m.name.as_ref() == method_name.as_ref())
            .cloned()
            .ok_or_else(|| {
                type_error(
                    TypeErrorKind::MethodNotFound {
                        method: Arc::clone(method_name),
                        ty: receiver_ty.clone(),
                    },
                    span,
                )
            })?;
        let slot = trait_def.dictionary_slot(method_name).unwrap_or_default();

        // Type against the trait signature with Self = the parameter, the
        // method's own generics instantiated fresh for this call. A method
        // with its own trait bounds (`fn tag<U: Eq>`) threads its own trailing
        // dictionaries: the dictionary slot — a concrete impl's `FunctionRef`
        // or a conditional impl's closure — accepts them after the value
        // arguments, so record them here (there is no impl-level dictionary at
        // this site; the enclosing function's dictionary already encapsulates
        // that). Solved on the dictionary schedule like a concrete-receiver
        // call.
        let (params, ret, abilities, type_var_map) =
            self.instantiate_trait_method_mapped(&method_def, receiver_ty);
        let method_dicts = self.resolve_method_bound_dicts(&method_def, &type_var_map);
        *dicts =
            self.record_trait_dispatch_dicts(None, receiver_ty, method_dicts, &bound.name, span);
        if args.len() != params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: params.len(),
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in method call `{}.{method_name}`", bound.name)));
        }
        for (i, (arg, param_ty)) in args.iter_mut().zip(params.iter()).enumerate() {
            // Seed an unannotated lambda argument from the parameter's
            // instantiated type (bidirectional checking).
            let arg_ty = self.infer_expr_expecting(env, arg, Some(param_ty))?;
            if let Err(e) = self.unify(&arg_ty, param_ty, span) {
                return Err(
                    e.with_context(format!("in argument {} of method `{method_name}`", i + 1))
                );
            }
        }

        let abilities = self.apply_abilities(&abilities);
        self.require_abilities(&abilities);

        *resolved_method = Some(ResolvedMethod::DictSlot { dict_index, slot });
        Ok(self.apply(&ret))
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
