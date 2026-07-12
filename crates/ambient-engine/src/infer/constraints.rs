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
            self.pending_constraints.push(PendingConstraint {
                ty,
                bound: bound.clone(),
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
    /// method that takes no dictionaries keeps its plain arity.
    pub(crate) fn record_conditional_impl_dicts(
        &mut self,
        target: Option<&Type>,
        bounds: &[(Arc<str>, TraitBound)],
        receiver_ty: &Type,
        span: (u32, u32),
    ) -> Option<Dicts> {
        if bounds.is_empty() {
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
    fn solve_bound(
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

        // A rigid parameter satisfies a bound iff the enclosing item
        // declares it; the dictionary forwards from the enclosing
        // dictionary parameter of the same (param, trait).
        if let Type::Param(name) = &ty {
            if let Some(dict_index) = self.bound_param_index(name, bound.trait_uuid) {
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

        // Concrete types satisfy a bound through an impl in the build.
        if let Some(type_uuid) = super::inherent::impl_key_for(&ty).and_then(|k| k.uuid())
            && let Some(imp) = self
                .trait_registry
                .get_impl(bound.trait_uuid, type_uuid)
                .cloned()
        {
            let Some(trait_def) = self.trait_registry.get_trait(bound.trait_uuid).cloned() else {
                return Err(Box::new(TypeError::new(
                    TypeErrorKind::UnknownTrait {
                        name: Arc::clone(&bound.name),
                    },
                    span,
                )));
            };
            if imp.is_generic {
                return self.solve_generic_impl(&ty, bound, span, depth, &imp, &trait_def);
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
            return Ok(DictSource::Impl { symbols });
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

        Err(Box::new(TypeError::new(
            TypeErrorKind::BoundNotSatisfied {
                ty,
                trait_name: Arc::clone(&bound.name),
            },
            span,
        )))
    }

    /// Solve `ty: bound` through a conditional impl (`impl<T: Eq> Eq for
    /// Pair<T>`): unify the impl's target shape against `ty` to recover its
    /// type-parameter assignments, recursively solve the impl's own bounds
    /// against those assignments, and describe the closure-built dictionary.
    fn solve_generic_impl(
        &mut self,
        ty: &Type,
        bound: &TraitBound,
        span: (u32, u32),
        depth: u32,
        imp: &crate::types::TraitImpl,
        trait_def: &crate::types::TraitDef,
    ) -> Result<DictSource, BoxedTypeError> {
        let not_satisfied = || {
            Box::new(TypeError::new(
                TypeErrorKind::BoundNotSatisfied {
                    ty: ty.clone(),
                    trait_name: Arc::clone(&bound.name),
                },
                span,
            ))
        };

        // Recover the impl's type-parameter assignments (`T -> Money` for
        // `Pair<T>` matched against `Pair<Money>`). A target that keys on the
        // same head uuid but a different instantiation (`Option<Number>` vs a
        // required `Option<String>`) fails to match — coherence granularity
        // is the head, precision is here.
        let target = imp.target.as_ref().ok_or_else(not_satisfied)?;
        let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
        if !match_target(target, ty, &mut subst) {
            return Err(not_satisfied());
        }

        // Solve each of the impl's own bounds against the recovered
        // assignment, in dictionary-parameter order — one inner dictionary
        // per bound, which every method closure forwards.
        let mut inner = Vec::with_capacity(imp.bounds.len());
        for (param, inner_bound) in &imp.bounds {
            let assigned = subst.get(param).cloned().ok_or_else(not_satisfied)?;
            inner.push(self.solve_bound(&assigned, inner_bound, span, depth + 1)?);
        }

        // One dictionary slot per trait method, in dictionary order — the
        // impl-method symbol plus how many value arguments the slot forwards
        // (receiver included) before the captured inner dictionaries.
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
                })
            })
            .collect::<Result<Vec<_>, BoxedTypeError>>()?;

        Ok(DictSource::Generic { methods, inner })
    }

    /// The dictionary-parameter index of `(param, trait)` in the enclosing
    /// item, if the item declares that bound.
    pub(crate) fn bound_param_index(&self, param: &str, trait_uuid: uuid::Uuid) -> Option<usize> {
        self.current_bound_params
            .iter()
            .position(|(name, bound)| name.as_ref() == param && bound.trait_uuid == trait_uuid)
    }

    /// Resolve an item's declared bounds (`<T: Eq + Ord>`) into the
    /// dictionary-parameter list. The order and dedup come from
    /// [`crate::ast::dict_params`] — the same authority the compiler
    /// allocates hidden parameters from — so checker indices and compiled
    /// slots can never disagree. Unknown trait names surface as errors and
    /// are skipped; the module doesn't compile in that case, so the index
    /// skew is harmless.
    pub(crate) fn resolve_bound_params(
        &mut self,
        type_params: &[crate::ast::TypeParam],
        errors: &mut Vec<BoxedTypeError>,
    ) -> Vec<(Arc<str>, TraitBound)> {
        let span = type_params
            .first()
            .map_or((0, 0), |tp| (tp.span.start, tp.span.end));
        let mut out = Vec::new();
        for (param, bound_name) in crate::ast::dict_params(type_params) {
            let Some(trait_uuid) = self.trait_registry.lookup_trait(&bound_name) else {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::UnknownTrait {
                        name: Arc::clone(&bound_name),
                    },
                    span,
                )));
                continue;
            };
            out.push((
                param,
                TraitBound {
                    trait_uuid,
                    name: bound_name,
                },
            ));
        }
        out
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
        let solved = self.solve_dict_constraints(errors);
        finalize_dicts(body, &solved);
        // State-cell fingerprints settle on the same schedule: render the
        // instantiated cell types now that the body's inference is done.
        let fingerprints = self.solve_fingerprints(errors);
        super::fingerprints::finalize_fingerprints(body, &fingerprints);
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
fn match_target(pattern: &Type, concrete: &Type, subst: &mut HashMap<Arc<str>, Type>) -> bool {
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
