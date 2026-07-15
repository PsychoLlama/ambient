//! Trait-bound constraint recording and solving.
//!
//! When a bounded scheme (`fn contains<T: Eq>(...)`) is instantiated, each
//! bound becomes a [`PendingConstraint`] on the fresh instantiation
//! variable, and the instantiating expression is annotated
//! [`Dicts::Pending`] with a group id. The variables usually resolve only
//! as inference proceeds through the enclosing body, so solving is
//! deferred: [`Infer::solve_dict_constraints`] runs once per item body,
//! after inference settles, resolves each constraint to a [`DictSource`],
//! and [`finalize_dicts`] rewrites the pending annotations to their solved
//! sources for the compiler.
//!
//! Solving a constraint `τ: Trait`:
//! - `τ` is a rigid [`Type::Param`] — the enclosing item must itself
//!   declare that bound; the dictionary is forwarded from the enclosing
//!   dictionary parameter ([`DictSource::Param`]).
//! - `τ` is a concrete type with a nominal identity — the build must
//!   contain `impl Trait for τ`; the dictionary is that impl's method
//!   symbols in dictionary order ([`DictSource::Impl`]), which the
//!   compiler links by content hash exactly like direct calls.
//! - anything else (an unresolved variable, a structural type) is an
//!   error.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{DictSource, Dicts, GenericDictMethod, walk_exprs_mut};
use crate::types::{TraitBound, Type};

/// How deep the solver follows conditional impls before giving up. A
/// self-referential impl (`impl<T: Eq> Eq for Pair<Pair<T>>` applied to an
/// ever-growing type) would otherwise loop forever; a legitimate program's
/// nesting is bounded by the source's finite type, well under this.
const MAX_SOLVE_DEPTH: u32 = 64;

use super::Infer;
use super::error::{BoxedTypeError, TypeError, TypeErrorKind};

/// A conditional (generic) impl's dictionary contribution at a trait-dispatch
/// call site: its applied target shape (used to recover type-parameter
/// assignments from the receiver) and its own bounds, in `dict_params` order.
type ConditionalImplDicts = (Option<Type>, Vec<(Arc<str>, TraitBound)>);

/// A deferred "does this generic impl's applied target cover the receiver?"
/// check. Recorded at a direct-dispatch site when the matched impl is generic
/// with an applied target but contributes no dictionary (a bound-less applied
/// impl like `impl Eq for Option<Number>`, whose bounded cousins are already
/// covered by their poisoned dictionary constraints). Solved once the body
/// settles, so the receiver's instantiation is as resolved as it will be.
#[derive(Debug)]
pub(crate) struct CoverageObligation {
    /// The impl's applied target shape, carrying the impl's [`Type::Param`]s.
    pub target: Type,
    /// The dispatch receiver whose instantiation must match `target`.
    pub receiver: Type,
    /// The trait's name, for the diagnostic.
    pub trait_name: Arc<str>,
    /// Span of the dispatch site.
    pub span: (u32, u32),
}

/// One unresolved `τ: Trait` obligation from instantiating a bounded scheme.
#[derive(Debug)]
pub(crate) struct PendingConstraint {
    /// The instantiation variable (or type) the bound applies to.
    pub ty: Type,
    /// The required trait.
    pub bound: TraitBound,
    /// The [`Dicts::Pending`] group this constraint belongs to.
    pub group: u32,
    /// Position within the group (the scheme's bound order).
    pub index: usize,
    /// Span of the instantiating expression, for diagnostics.
    pub span: (u32, u32),
}

impl Infer {
    /// Record the bound obligations of a just-instantiated scheme and hand
    /// back the [`Dicts::Pending`] annotation for the instantiating
    /// expression. `instantiated` maps the scheme's quantified vars to
    /// their fresh instantiation types.
    pub(crate) fn record_bound_constraints(
        &mut self,
        bounds: &[(crate::types::TypeVarId, TraitBound)],
        instantiated: &HashMap<crate::types::TypeVarId, Type>,
        span: (u32, u32),
    ) -> Dicts {
        let group = self.next_dict_group;
        self.next_dict_group += 1;
        for (index, (var, bound)) in bounds.iter().enumerate() {
            // A bound var always has an instantiation entry; `Error` keeps
            // a checker bug from panicking and surfaces at solve time.
            let ty = instantiated.get(var).cloned().unwrap_or(Type::Error);
            // A scheme bound's arguments quantify over the scheme's own
            // variables (`fn f<U, T: From<U>>` stores `From<'q>`), so they
            // instantiate alongside the bound variable itself.
            let mut bound = bound.clone();
            if !bound.args.is_empty() {
                let no_abilities = HashMap::new();
                bound.args = bound
                    .args
                    .iter()
                    .map(|a| a.substitute_all(instantiated, &no_abilities))
                    .collect();
            }
            self.pending_constraints.push(PendingConstraint {
                ty,
                bound,
                group,
                index,
                span,
            });
        }
        Dicts::Pending(group)
    }

    /// Record the hidden dictionaries a *direct* call to a conditional impl's
    /// method needs. `impl<T: Eq> Eq for Pair<T>`'s `eq` takes one trailing
    /// dictionary (its `T: Eq` bound); a `pair.eq(other)` or `a == b` on a
    /// concrete `Pair<Money>` must supply it, exactly like a bounded-generic
    /// call — one pending constraint per impl bound, solved against the
    /// receiver's matched type arguments once the body settles.
    ///
    /// Returns `None` (no annotation) when the impl declares no bounds, so a
    /// method that takes no dictionaries keeps its plain arity. A bound-less
    /// *applied* impl still needs its instantiation checked — coherence found
    /// it by head, but `impl Eq for Option<Number>` does not cover an
    /// `Option<String>` receiver — so a coverage obligation is recorded in
    /// that case (see [`Infer::record_impl_coverage`]).
    pub(crate) fn record_conditional_impl_dicts(
        &mut self,
        target: Option<&Type>,
        bounds: &[(Arc<str>, TraitBound)],
        receiver_ty: &Type,
        trait_name: &Arc<str>,
        span: (u32, u32),
    ) -> Option<Dicts> {
        if bounds.is_empty() {
            self.record_impl_coverage(target, receiver_ty, trait_name, span);
            return None;
        }
        let ty = self.apply(receiver_ty);
        let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
        let matched = target.is_some_and(|t| match_target(t, &ty, &mut subst));
        let group = self.next_dict_group;
        self.next_dict_group += 1;
        for (index, (param, bound)) in bounds.iter().enumerate() {
            // A matched param yields the concrete assignment (`T -> Money`);
            // a failed match poisons the constraint with the receiver so
            // solving reports the unsatisfied bound rather than silently
            // dropping a required dictionary.
            let cty = matched
                .then(|| subst.get(param).cloned())
                .flatten()
                .unwrap_or_else(|| ty.clone());
            self.pending_constraints.push(PendingConstraint {
                ty: cty,
                bound: bound.clone(),
                group,
                index,
                span,
            });
        }
        Some(Dicts::Pending(group))
    }

    /// Record the hidden dictionaries a *direct* trait-dispatched call needs,
    /// as a single ordered group: the receiver's conditional-impl (impl-level)
    /// bounds first, then the method's own method-level bounds — exactly the
    /// combined order [`crate::compiler`] allocates the impl method's trailing
    /// dictionary parameters in (`alloc_dict_locals(impl ++ method)`).
    ///
    /// `generic_impl` is `Some((target, impl bounds))` when the receiver's impl
    /// is conditional; `method_bounds` is the method's own bounds already paired
    /// with their fresh instantiation variables. Returns `None` (plain arity)
    /// when neither contributes a dictionary.
    pub(crate) fn record_trait_dispatch_dicts(
        &mut self,
        generic_impl: Option<ConditionalImplDicts>,
        receiver_ty: &Type,
        method_bounds: Vec<(Type, TraitBound)>,
        trait_name: &Arc<str>,
        span: (u32, u32),
    ) -> Option<Dicts> {
        let impl_bound_count = generic_impl.as_ref().map_or(0, |(_, b)| b.len());
        if impl_bound_count == 0 && method_bounds.is_empty() {
            // No dictionaries to thread, but a bound-less *applied* impl still
            // needs its instantiation checked: coherence found it by head, yet
            // `impl Eq for Option<Number>` does not cover an `Option<String>`
            // receiver.
            if let Some((target, _)) = &generic_impl {
                self.record_impl_coverage(target.as_ref(), receiver_ty, trait_name, span);
            }
            return None;
        }
        let group = self.next_dict_group;
        self.next_dict_group += 1;
        let mut index = 0;

        if let Some((target, bounds)) = generic_impl {
            let ty = self.apply(receiver_ty);
            let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
            let matched = target
                .as_ref()
                .is_some_and(|t| match_target(t, &ty, &mut subst));
            for (param, bound) in &bounds {
                // A matched param yields its concrete assignment (`T -> Money`);
                // a failed match poisons the constraint with the receiver so
                // solving reports the unsatisfied bound rather than silently
                // dropping a required dictionary.
                let cty = matched
                    .then(|| subst.get(param).cloned())
                    .flatten()
                    .unwrap_or_else(|| ty.clone());
                self.pending_constraints.push(PendingConstraint {
                    ty: cty,
                    bound: bound.clone(),
                    group,
                    index,
                    span,
                });
                index += 1;
            }
        }

        for (ty, bound) in method_bounds {
            self.pending_constraints.push(PendingConstraint {
                ty,
                bound,
                group,
                index,
                span,
            });
            index += 1;
        }

        Some(Dicts::Pending(group))
    }

    /// Record a deferred coverage obligation for a generic impl matched at a
    /// direct-dispatch site. A no-op when the impl has no applied `target` (a
    /// plain `impl Eq for Money` covers everything it matches). Otherwise the
    /// obligation is solved once the body settles, so the receiver's
    /// instantiation is resolved before [`match_target`] runs — mirroring how
    /// the bounded path defers its constraints, and avoiding a false rejection
    /// of a receiver whose type is pinned later in the body.
    pub(crate) fn record_impl_coverage(
        &mut self,
        target: Option<&Type>,
        receiver_ty: &Type,
        trait_name: &Arc<str>,
        span: (u32, u32),
    ) {
        if let Some(target) = target {
            self.pending_coverage.push(CoverageObligation {
                target: target.clone(),
                receiver: receiver_ty.clone(),
                trait_name: Arc::clone(trait_name),
                span,
            });
        }
    }

    /// Discharge every coverage obligation recorded since the last call. Each
    /// applies the receiver (now as resolved as it will be) and matches it
    /// against the impl's applied target; a mismatch is an
    /// [`TypeErrorKind::ImplInstantiationNotCovered`] error.
    pub(crate) fn solve_coverage(&mut self, errors: &mut Vec<BoxedTypeError>) {
        for obligation in std::mem::take(&mut self.pending_coverage) {
            let ty = self.apply(&obligation.receiver);
            let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
            if !match_target(&obligation.target, &ty, &mut subst) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::ImplInstantiationNotCovered {
                        // Render the receiver's *applied* surface form
                        // (`Pair<String>`, not the bare head `Pair`): a generic
                        // struct's arguments were substituted into its record
                        // body, so `Display` alone can only show the head.
                        ty: self.applied_nominal_display(&ty),
                        trait_name: Arc::clone(&obligation.trait_name),
                    },
                    obligation.span,
                )));
            }
        }
    }

    /// Reconstruct a nominal *struct* type's applied surface form for a
    /// diagnostic. A generic struct's applied form (`Pair<String>`) reaches
    /// the checker as a bare [`Type::Nominal`] whose arguments were already
    /// substituted into its record body ([`substitute_named`]), so
    /// [`Type`]'s `Display` — which has no environment — can only print the
    /// head. This reverses that substitution against the struct's declared
    /// body (recovered from the [`AliasTarget::GenericStruct`] table by uuid,
    /// name-independent) to recover the arguments and rebuild the applied
    /// `Named<…>` shape `Display` renders in full. Recurses into recovered
    /// arguments so a nested `Pair<Pair<Money>>` renders in full too.
    /// Non-struct types, and structs whose body we can't reverse, pass
    /// through unchanged (the bare-head `Display` is the graceful fallback).
    ///
    /// This is a human-facing rendering path used only when building an error;
    /// content hashes and the on-disk store render through
    /// [`CanonicalTypeRenderer`](crate::ability_resolver::CanonicalTypeRenderer)
    /// instead, so this never touches a persisted or hashed form.
    ///
    /// [`substitute_named`]: super::check::subst::substitute_named
    pub(crate) fn applied_nominal_display(&self, ty: &Type) -> Type {
        let Type::Nominal(nom) = ty else {
            return ty.clone();
        };
        for target in self.type_aliases.values() {
            let super::AliasTarget::GenericStruct {
                type_params,
                body: Type::Nominal(body),
            } = target
            else {
                continue;
            };
            if body.uuid != nom.uuid {
                continue;
            }
            let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
            if invert_named(&body.inner, &nom.inner, type_params, &mut subst) {
                let args = type_params
                    .iter()
                    .map(|param| {
                        self.applied_nominal_display(subst.get(param).unwrap_or(&Type::Hole))
                    })
                    .collect();
                let name = nom.name.clone().unwrap_or_else(|| Arc::from("_"));
                return Type::Named(crate::types::NamedType::with_identity(
                    name,
                    args,
                    Some(nom.uuid),
                ));
            }
        }
        ty.clone()
    }

    /// Solve every constraint recorded since the last call, producing the
    /// per-group dictionary sources. Runs after an item body is fully
    /// inferred (while the item's `current_bound_params` context is still
    /// installed) so instantiation variables are as resolved as they will
    /// ever be.
    pub(crate) fn solve_dict_constraints(
        &mut self,
        errors: &mut Vec<BoxedTypeError>,
    ) -> HashMap<u32, Vec<DictSource>> {
        let pending = std::mem::take(&mut self.pending_constraints);
        let mut groups: HashMap<u32, Vec<(usize, DictSource)>> = HashMap::new();

        for constraint in pending {
            match self.solve_one(&constraint) {
                Ok(source) => {
                    groups
                        .entry(constraint.group)
                        .or_default()
                        .push((constraint.index, source));
                }
                Err(e) => {
                    errors.push(e);
                    // Keep the group's arity intact so the compiler still
                    // sees one source per bound; Error-typed modules never
                    // compile anyway.
                    groups
                        .entry(constraint.group)
                        .or_default()
                        .push((constraint.index, DictSource::Impl { symbols: vec![] }));
                }
            }
        }

        groups
            .into_iter()
            .map(|(group, mut sources)| {
                sources.sort_by_key(|(index, _)| *index);
                (group, sources.into_iter().map(|(_, s)| s).collect())
            })
            .collect()
    }

    /// Solve one `τ: Trait` obligation from a bounded scheme instantiation.
    fn solve_one(&mut self, constraint: &PendingConstraint) -> Result<DictSource, BoxedTypeError> {
        self.solve_bound(&constraint.ty, &constraint.bound, constraint.span, 0)
    }

    /// Solve a `ty: bound` obligation, recursing through conditional impls.
    ///
    /// `depth` counts the conditional-impl nesting so a non-terminating chain
    /// (`impl<T: Eq> Eq for Pair<Pair<T>>` applied to an ever-growing type)
    /// is cut off with a clear error rather than looping.
    pub(crate) fn solve_bound(
        &mut self,
        ty: &Type,
        bound: &TraitBound,
        span: (u32, u32),
        depth: u32,
    ) -> Result<DictSource, BoxedTypeError> {
        if depth > MAX_SOLVE_DEPTH {
            return Err(Box::new(TypeError::new(
                TypeErrorKind::DictSolveDepthLimit {
                    ty: self.apply(ty),
                    trait_name: Arc::clone(&bound.name),
                },
                span,
            )));
        }
        let ty = self.apply(ty);
        let mut bound = bound.clone();
        bound.args = bound.args.iter().map(|a| self.apply(a)).collect();
        let bound = &bound;

        // A rigid parameter satisfies a bound iff the enclosing item
        // declares it; the dictionary forwards from the enclosing
        // dictionary parameter of the same (param, trait, args).
        if let Type::Param(name) = &ty {
            if let Some(dict_index) = self.bound_param_index(name, bound.trait_uuid, &bound.args) {
                return Ok(DictSource::Param { dict_index });
            }
            return Err(Box::new(TypeError::new(
                TypeErrorKind::MissingParamBound {
                    param: Arc::clone(name),
                    trait_name: Arc::clone(&bound.name),
                },
                span,
            )));
        }

        // Concrete types satisfy a bound through an impl in the build:
        // among the (argument-differing) impls of this trait for this type,
        // the one whose target and trait arguments match the requirement.
        if let Some(type_uuid) = super::inherent::impl_key_for(&ty).and_then(|k| k.uuid()) {
            let Some(trait_def) = self.trait_registry.get_trait(bound.trait_uuid).cloned() else {
                return Err(Box::new(TypeError::new(
                    TypeErrorKind::UnknownTrait {
                        name: Arc::clone(&bound.name),
                    },
                    span,
                )));
            };
            let candidates: Vec<crate::types::TraitImpl> = self
                .trait_registry
                .impls_of(bound.trait_uuid, type_uuid)
                .into_iter()
                .cloned()
                .collect();
            for imp in &candidates {
                if let Some(source) =
                    self.solve_via_impl(&ty, bound, span, depth, imp, &trait_def)?
                {
                    return Ok(source);
                }
            }
        }

        // The conversion bridges: `S: Into<T>` is satisfiable by
        // `impl From<S> for T`, and `S: TryInto<T>` by
        // `impl TryFrom<S> for T`. Sound at runtime because each pair is
        // pinned to the same dictionary shape — a 1-tuple of one 1-argument
        // function — so a `From` dictionary *is* an `Into` dictionary (and
        // likewise for the fallible pair). Anchored on the reserved uuids,
        // like operator desugaring.
        let bridge_source = match bound.trait_uuid {
            id if id == crate::types::TRAIT_INTO_UUID => {
                Some((crate::types::TRAIT_FROM_UUID, "From"))
            }
            id if id == crate::types::TRAIT_TRY_INTO_UUID => {
                Some((crate::types::TRAIT_TRY_FROM_UUID, "TryFrom"))
            }
            _ => None,
        };
        if let Some((from_uuid, from_name)) = bridge_source
            && let [target] = bound.args.as_slice()
            && let Some(source) = self.solve_into_via_from(
                &ty,
                &target.clone(),
                bound,
                span,
                depth,
                from_uuid,
                from_name,
            )?
        {
            return Ok(source);
        }

        if matches!(ty, Type::Var(_)) {
            return Err(Box::new(TypeError::new(
                TypeErrorKind::CannotInfer {
                    hint: format!(
                        "type argument constrained by `{}` (add an annotation)",
                        bound.name
                    ),
                },
                span,
            )));
        }

        // Render the arguments into the diagnostic (`Unwrap<Number>`), so an
        // args-mismatched impl reads as the missing instantiation it is.
        let display: Arc<str> = if bound.args.is_empty() {
            Arc::clone(&bound.name)
        } else {
            let args: Vec<String> = bound.args.iter().map(|a| format!("{a}")).collect();
            Arc::from(format!("{}<{}>", bound.name, args.join(", ")))
        };
        Err(Box::new(TypeError::new(
            TypeErrorKind::BoundNotSatisfied {
                ty,
                trait_name: display,
            },
            span,
        )))
    }

    /// Try to satisfy `ty: bound` through one candidate impl: match the
    /// impl's target and trait arguments against the required ones (one
    /// shared substitution binds any impl type parameters across both, so
    /// `impl<T> From<List<T>> for Set<T>` lines its `T`s up), and build the
    /// dictionary on success. `Ok(None)` means "this impl doesn't cover the
    /// requirement" — the caller tries the next candidate.
    fn solve_via_impl(
        &mut self,
        ty: &Type,
        bound: &TraitBound,
        span: (u32, u32),
        depth: u32,
        imp: &crate::types::TraitImpl,
        trait_def: &crate::types::TraitDef,
    ) -> Result<Option<DictSource>, BoxedTypeError> {
        if imp.trait_args.len() != bound.args.len() {
            return Ok(None);
        }
        let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
        // Only a conditional impl's target is matched (to recover its
        // parameter assignments and check coverage); a ground impl's
        // identity was already keyed by `type_uuid`, and its recorded
        // target may differ representationally from the receiver.
        if imp.is_generic
            && let Some(target) = &imp.target
            && !match_target(target, ty, &mut subst)
        {
            return Ok(None);
        }
        for (impl_arg, required) in imp.trait_args.iter().zip(&bound.args) {
            if !match_target(impl_arg, required, &mut subst) {
                return Ok(None);
            }
        }

        if imp.is_generic {
            return self
                .solve_generic_impl(bound, span, depth, imp, trait_def, &subst)
                .map(Some);
        }
        let symbols = trait_def
            .dictionary_order()
            .into_iter()
            .map(|idx| {
                let method_name = &trait_def.methods[idx].name;
                imp.methods.get(method_name).cloned().ok_or_else(|| {
                    Box::new(TypeError::new(
                        TypeErrorKind::BoundNotSatisfied {
                            ty: ty.clone(),
                            trait_name: Arc::clone(&bound.name),
                        },
                        span,
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(DictSource::Impl { symbols }))
    }

    /// Solve `ty: bound` through a conditional impl (`impl<T: Eq> Eq for
    /// Pair<T>`) whose target and trait arguments already matched, with
    /// `subst` carrying the recovered type-parameter assignments: solve the
    /// impl's own bounds against those assignments and describe the
    /// closure-built dictionary.
    fn solve_generic_impl(
        &mut self,
        bound: &TraitBound,
        span: (u32, u32),
        depth: u32,
        imp: &crate::types::TraitImpl,
        trait_def: &crate::types::TraitDef,
        subst: &HashMap<Arc<str>, Type>,
    ) -> Result<DictSource, BoxedTypeError> {
        let not_satisfied = || {
            Box::new(TypeError::new(
                TypeErrorKind::BoundNotSatisfied {
                    ty: imp.target.clone().unwrap_or(Type::Error),
                    trait_name: Arc::clone(&bound.name),
                },
                span,
            ))
        };

        // The caller already recovered the impl's type-parameter assignments
        // (`T -> Money` for `Pair<T>` matched against `Pair<Money>`, plus
        // any bound by the trait arguments). A target keying on the same
        // head uuid but a different instantiation failed to match there —
        // coherence granularity is the head, precision is the match.
        //
        // Solve each of the impl's own bounds against the recovered
        // assignment, in dictionary-parameter order — one inner dictionary
        // per bound, which every method closure forwards.
        let mut inner = Vec::with_capacity(imp.bounds.len());
        for (param, inner_bound) in &imp.bounds {
            let assigned = subst.get(param).cloned().ok_or_else(not_satisfied)?;
            // An inner bound's own arguments may reference the impl's
            // parameters (`impl<T, U: From<T>> …`); substitute the recovered
            // assignments before solving.
            let mut inner_bound = inner_bound.clone();
            if !inner_bound.args.is_empty() {
                inner_bound.args = inner_bound
                    .args
                    .iter()
                    .map(|a| substitute_rigid_params(a, subst))
                    .collect();
            }
            inner.push(self.solve_bound(&assigned, &inner_bound, span, depth + 1)?);
        }

        // One dictionary slot per trait method, in dictionary order — the
        // impl-method symbol, how many value arguments the slot forwards
        // (receiver included) before the captured inner dictionaries, and how
        // many of the method's *own* bound dictionaries (`fn m<U: Eq>`) follow
        // them. The method-dictionary count is the method's `method_bounds`
        // length — itself the single-authority `dict_params` list — so the
        // slot closure's arity matches what the bound-method call site pushes.
        let methods = trait_def
            .dictionary_order()
            .into_iter()
            .map(|idx| {
                let m = &trait_def.methods[idx];
                let symbol = imp
                    .methods
                    .get(&m.name)
                    .cloned()
                    .ok_or_else(not_satisfied)?;
                Ok(GenericDictMethod {
                    symbol,
                    arity: usize::from(m.has_self) + m.params.len(),
                    method_dict_count: m.method_bounds.len(),
                })
            })
            .collect::<Result<Vec<_>, BoxedTypeError>>()?;

        Ok(DictSource::Generic { methods, inner })
    }

    /// The dictionary-parameter index of `(param, trait, args)` in the
    /// enclosing item, if the item declares that bound. Arguments compare
    /// structurally after applying the current substitution on both sides,
    /// so `T: Into<String>` matches a requirement whose argument resolved to
    /// `String` through inference.
    pub(crate) fn bound_param_index(
        &mut self,
        param: &str,
        trait_uuid: uuid::Uuid,
        args: &[Type],
    ) -> Option<usize> {
        let required: Vec<Type> = args.iter().map(|a| self.apply(a)).collect();
        let declared: Vec<(usize, Vec<Type>)> = self
            .current_bound_params
            .iter()
            .enumerate()
            .filter(|(_, (name, bound))| {
                name.as_ref() == param
                    && bound.trait_uuid == trait_uuid
                    && bound.args.len() == required.len()
            })
            .map(|(i, (_, bound))| (i, bound.args.clone()))
            .collect();
        declared
            .into_iter()
            .find(|(_, args)| args.iter().zip(&required).all(|(a, b)| self.apply(a) == *b))
            .map(|(i, _)| i)
    }

    /// Resolve a trait *reference* — an impl header or a `T: Bound` — to its
    /// nominal uuid. Prefers the resolve pass's canonical [`Fqn`], mapped
    /// through the build-global table: scope-blind, so a foreign signature's
    /// bound resolves in the module that *defined* it and no consumer-side
    /// same-named trait can shadow it. Falls back to an in-scope bare-name
    /// lookup only when the reference was never resolved — a registry-less
    /// single-file/test check, where every trait is local and named.
    #[must_use]
    pub(crate) fn trait_uuid_of(&self, name: &crate::ast::QualifiedName) -> Option<uuid::Uuid> {
        match &name.resolved {
            Some(fqn) => self.trait_registry.uuid_for_fqn(fqn),
            None => self.trait_registry.lookup_trait(&name.name),
        }
    }

    /// Resolve an item's declared bounds (`<T: Eq + Ord>`) into the
    /// dictionary-parameter list. The order and dedup come from
    /// [`crate::ast::dict_params`] — the same authority the compiler
    /// allocates hidden parameters from — so checker indices and compiled
    /// slots can never disagree. Unknown trait references surface as errors
    /// and are skipped; the module doesn't compile in that case, so the
    /// index skew is harmless.
    pub(crate) fn resolve_bound_params(
        &mut self,
        type_params: &[crate::ast::TypeParam],
        errors: &mut Vec<BoxedTypeError>,
    ) -> Vec<(Arc<str>, TraitBound)> {
        let span = type_params
            .first()
            .map_or((0, 0), |tp| (tp.span.start, tp.span.end));
        // A bound's arguments may reference the declaration's own type
        // parameters (`fn f<U, T: From<U>>`); resolve them under those
        // params rigid so they stay `Type::Param`s, matching how the
        // enclosing body resolves every other annotation.
        let rigid: Vec<Arc<str>> = type_params
            .iter()
            .filter(|tp| !tp.is_ability)
            .map(|tp| Arc::clone(&tp.name))
            .collect();
        let mut out = Vec::new();
        for (param, bound) in crate::ast::dict_params(type_params) {
            let resolved = self.with_rigid_params(rigid.clone(), |infer| {
                infer.resolve_trait_ref(bound, span, errors)
            });
            if let Some(resolved) = resolved {
                out.push((param, resolved));
            }
        }
        out
    }

    /// Resolve one trait reference (`Eq`, `From<String>`) to a
    /// [`TraitBound`]: the trait identity via [`Self::trait_uuid_of`], the
    /// arguments through ordinary type resolution (under whatever rigid
    /// scope the caller installed), validated against the trait's declared
    /// parameter count. `None` (with an error pushed) for an unknown trait
    /// or an argument-count mismatch.
    pub(crate) fn resolve_trait_ref(
        &mut self,
        bound: &crate::ast::TraitRef,
        span: (u32, u32),
        errors: &mut Vec<BoxedTypeError>,
    ) -> Option<TraitBound> {
        let Some(trait_uuid) = self.trait_uuid_of(&bound.name) else {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::UnknownTrait {
                    name: Arc::clone(&bound.name.name),
                },
                span,
            )));
            return None;
        };
        let args: Vec<Type> = bound.args.iter().map(|a| self.resolve_holes(a)).collect();
        if let Some(def) = self.trait_registry.get_trait(trait_uuid)
            && def.type_params.len() != args.len()
        {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::TraitArityMismatch {
                    trait_name: Arc::clone(&bound.name.name),
                    expected: def.type_params.len(),
                    found: args.len(),
                },
                span,
            )));
            return None;
        }
        Some(TraitBound {
            trait_uuid,
            name: Arc::clone(&bound.name.name),
            args,
        })
    }

    /// Install an item's dictionary-parameter context around `f` (composes
    /// with [`Infer::with_rigid_params`], which handles the *names*).
    pub(crate) fn with_bound_params<T>(
        &mut self,
        bounds: Vec<(Arc<str>, TraitBound)>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = std::mem::replace(&mut self.current_bound_params, bounds);
        let result = f(self);
        self.current_bound_params = saved;
        result
    }

    /// Solve the constraints recorded while checking one item body and
    /// finalize the body's dictionary annotations. Call at the end of a
    /// body check, while the item's `current_bound_params` context is
    /// still installed.
    pub(crate) fn finish_body_constraints(
        &mut self,
        body: &mut crate::ast::Expr,
        errors: &mut Vec<BoxedTypeError>,
    ) {
        // Deferred conversion selections settle first: the chosen impl may
        // record fresh dictionary constraints, which the dictionary solve
        // below then resolves in the same pass.
        let selections = self.solve_method_selections(errors);
        super::conversions::finalize_method_selections(body, &selections);
        let solved = self.solve_dict_constraints(errors);
        finalize_dicts(body, &solved);
        // Applied-impl coverage settles on the same schedule: the receiver's
        // instantiation is now resolved, so an uncovered match is caught here
        // rather than silently misdispatching.
        self.solve_coverage(errors);
        // State-cell fingerprints settle on the same schedule: render the
        // instantiated cell types now that the body's inference is done.
        let fingerprints = self.solve_fingerprints(errors);
        super::fingerprints::finalize_fingerprints(body, &fingerprints);
    }
}

/// Substitute [`Type::Param`]s by name — the inverse direction of
/// [`match_target`]'s binding: once a conditional impl's parameters are
/// assigned (`T -> Money`), rewrite a type that *mentions* those parameters
/// (an inner bound's trait argument, `From<T>`) to its concrete form.
/// Unassigned params pass through unchanged.
pub(crate) fn substitute_rigid_params(ty: &Type, subst: &HashMap<Arc<str>, Type>) -> Type {
    match ty {
        Type::Param(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Named(n) => Type::Named(
            n.map_args(
                n.args
                    .iter()
                    .map(|a| substitute_rigid_params(a, subst))
                    .collect(),
            ),
        ),
        Type::Nominal(nom) => {
            Type::Nominal(nom.map_inner(substitute_rigid_params(&nom.inner, subst)))
        }
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_rigid_params(e, subst))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_rigid_params(t, subst)))
                .collect(),
        )),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_rigid_params(p, subst))
                .collect(),
            substitute_rigid_params(&f.ret, subst),
            f.abilities.clone(),
        ),
        // A projection over an assigned parameter (`T::Error` with
        // `T -> Money`) keeps the projection form; whoever holds the impl's
        // associated bindings eliminates it. Substituting only the base
        // keeps this walk a pure param rewrite.
        Type::Projection(p) => p.with_base(substitute_rigid_params(&p.base, subst)),
        _ => ty.clone(),
    }
}

/// One-directional structural match of a conditional impl's target shape
/// (`pattern`, carrying the impl's [`Type::Param`]s) against a concrete
/// `concrete` type, binding each param to the type it lines up with.
///
/// This is *matching*, not full unification: only `pattern`'s params bind,
/// and a param that recurs must line up with an equal type each time
/// (`Pair<T>` against `Pair<Money>` binds `T = Money` consistently). A
/// structural mismatch — a different head uuid, arity, or field set — is a
/// clean `false`, which the caller reports as an unsatisfied bound.
pub(crate) fn match_target(
    pattern: &Type,
    concrete: &Type,
    subst: &mut HashMap<Arc<str>, Type>,
) -> bool {
    match (pattern, concrete) {
        (Type::Param(name), _) => {
            if let Some(existing) = subst.get(name) {
                existing == concrete
            } else {
                subst.insert(Arc::clone(name), concrete.clone());
                true
            }
        }
        (Type::Named(p), Type::Named(c)) => {
            let head_ok = match (p.uuid, c.uuid) {
                (Some(a), Some(b)) => a == b,
                _ => p.name == c.name,
            };
            head_ok
                && p.args.len() == c.args.len()
                && p.args
                    .iter()
                    .zip(&c.args)
                    .all(|(a, b)| match_target(a, b, subst))
        }
        (Type::Nominal(p), Type::Nominal(c)) => {
            p.uuid == c.uuid && match_target(&p.inner, &c.inner, subst)
        }
        (Type::Record(p), Type::Record(c)) => {
            p.fields.len() == c.fields.len()
                && p.fields
                    .iter()
                    .zip(&c.fields)
                    .all(|((pn, pt), (cn, ct))| pn == cn && match_target(pt, ct, subst))
        }
        (Type::Tuple(p), Type::Tuple(c)) => {
            p.len() == c.len() && p.iter().zip(c).all(|(a, b)| match_target(a, b, subst))
        }
        (Type::Function(p), Type::Function(c)) => {
            p.params.len() == c.params.len()
                && p.params
                    .iter()
                    .zip(&c.params)
                    .all(|(a, b)| match_target(a, b, subst))
                && match_target(&p.ret, &c.ret, subst)
        }
        _ => pattern == concrete,
    }
}

/// Invert [`substitute_named`] for diagnostics: match a generic struct's
/// declared body `pattern` (its fields written as bare `Named(param)`
/// placeholders) against a concrete instantiation, binding each declared
/// `param` to the type it lines up with. The dual of [`match_target`], which
/// binds [`Type::Param`]s; here the binders are the placeholder `Named`s a
/// fielded generic struct's body carries. A structural mismatch is a clean
/// `false`, leaving the caller to fall back to the bare-head rendering.
///
/// [`substitute_named`]: super::check::subst::substitute_named
fn invert_named(
    pattern: &Type,
    concrete: &Type,
    params: &[Arc<str>],
    subst: &mut HashMap<Arc<str>, Type>,
) -> bool {
    // A bare placeholder naming one of the struct's parameters binds it.
    if let Type::Named(p) = pattern
        && p.args.is_empty()
        && p.uuid.is_none()
        && params.iter().any(|q| q == &p.name)
    {
        return if let Some(existing) = subst.get(&p.name) {
            existing == concrete
        } else {
            subst.insert(Arc::clone(&p.name), concrete.clone());
            true
        };
    }
    match (pattern, concrete) {
        (Type::Named(p), Type::Named(c)) => {
            let head_ok = match (p.uuid, c.uuid) {
                (Some(a), Some(b)) => a == b,
                _ => p.name == c.name,
            };
            head_ok
                && p.args.len() == c.args.len()
                && p.args
                    .iter()
                    .zip(&c.args)
                    .all(|(a, b)| invert_named(a, b, params, subst))
        }
        (Type::Nominal(p), Type::Nominal(c)) => {
            p.uuid == c.uuid && invert_named(&p.inner, &c.inner, params, subst)
        }
        (Type::Record(p), Type::Record(c)) => {
            p.fields.len() == c.fields.len()
                && p.fields
                    .iter()
                    .zip(&c.fields)
                    .all(|((pn, pt), (cn, ct))| pn == cn && invert_named(pt, ct, params, subst))
        }
        (Type::Tuple(p), Type::Tuple(c)) => {
            p.len() == c.len()
                && p.iter()
                    .zip(c)
                    .all(|(a, b)| invert_named(a, b, params, subst))
        }
        (Type::Function(p), Type::Function(c)) => {
            p.params.len() == c.params.len()
                && p.params
                    .iter()
                    .zip(&c.params)
                    .all(|(a, b)| invert_named(a, b, params, subst))
                && invert_named(&p.ret, &c.ret, params, subst)
        }
        _ => pattern == concrete,
    }
}

/// Rewrite every [`Dicts::Pending`] annotation in `expr` to its solved
/// sources. A group missing from `solved` (a checker bug) is left pending;
/// the compiler reports it as an internal error rather than miscompiling.
pub(crate) fn finalize_dicts(expr: &mut crate::ast::Expr, solved: &HashMap<u32, Vec<DictSource>>) {
    walk_exprs_mut(expr, &mut |e| {
        if let Some(Dicts::Pending(group)) = &e.dicts
            && let Some(sources) = solved.get(group)
        {
            e.dicts = Some(Dicts::Resolved(sources.clone()));
        }
    });
}
