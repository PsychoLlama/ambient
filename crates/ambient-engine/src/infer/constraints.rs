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

use crate::ast::{DictSource, Dicts, walk_exprs_mut};
use crate::types::{TraitBound, Type};

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

    /// Solve one `τ: Trait` obligation.
    fn solve_one(&mut self, constraint: &PendingConstraint) -> Result<DictSource, BoxedTypeError> {
        let ty = self.apply(&constraint.ty);
        let bound = &constraint.bound;

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
                constraint.span,
            )));
        }

        // Concrete types satisfy a bound through an impl in the build.
        if let Some(type_uuid) = super::inherent::impl_key_for(&ty).and_then(|k| k.uuid())
            && let Some(imp) = self.trait_registry.get_impl(bound.trait_uuid, type_uuid)
        {
            if imp.is_generic {
                return Err(Box::new(TypeError::new(
                    TypeErrorKind::GenericImplAsDictionary {
                        trait_name: Arc::clone(&bound.name),
                        ty: ty.clone(),
                    },
                    constraint.span,
                )));
            }
            let Some(trait_def) = self.trait_registry.get_trait(bound.trait_uuid) else {
                return Err(Box::new(TypeError::new(
                    TypeErrorKind::UnknownTrait {
                        name: Arc::clone(&bound.name),
                    },
                    constraint.span,
                )));
            };
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
                            constraint.span,
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
                constraint.span,
            )));
        }

        Err(Box::new(TypeError::new(
            TypeErrorKind::BoundNotSatisfied {
                ty,
                trait_name: Arc::clone(&bound.name),
            },
            constraint.span,
        )))
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
