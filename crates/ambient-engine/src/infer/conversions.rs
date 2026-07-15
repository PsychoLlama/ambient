//! Conversion-call resolution: selecting among argument-differing impls.
//!
//! A parameterized trait can have several impls for one receiver type
//! (`impl From<Number> for Money` and `impl From<String> for Money`), so a
//! zero-argument method call like `x.into()` cannot always pick its impl
//! from the receiver alone — the *result* type decides. When exactly one
//! candidate exists it resolves immediately (and pins the call's type);
//! otherwise the call is annotated [`ResolvedMethod::Pending`] and selection
//! defers to [`Infer::solve_method_selections`], run by
//! `finish_body_constraints` once the body's inference has settled the
//! result type — the same schedule dictionary constraints solve on.
//!
//! The candidate set for `into` is the union of direct `Into` impls on the
//! receiver and the *bridge*: every `impl From<S> for T` is an `Into<T>`
//! candidate for `S`. The bridge is anchored on the reserved
//! [`TRAIT_FROM_UUID`]/[`TRAIT_INTO_UUID`] pair and is sound at runtime
//! because both traits are pinned to the same dictionary/method shape — one
//! 1-argument function — so `from`'s symbol serves as `into`'s.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Dicts, ResolvedMethod, walk_exprs_mut};
use crate::types::{TRAIT_FROM_UUID, TraitImpl, Type, trait_arg_head};

use super::Infer;
use super::constraints::{match_target, substitute_rigid_params};
use super::error::{BoxedTypeError, TypeError, TypeErrorKind};

/// Which type the chosen impl's target must cover when recording the call's
/// hidden dictionaries (a conditional impl's own bounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchReceiver {
    /// The impl is on the receiver's type (`impl Into<T> for S` — dot
    /// dispatch): match its target against the receiver.
    Receiver,
    /// The impl is on the *produced* type (`impl From<S> for T`, bridged to
    /// `Into`): match its target against the call's result type.
    Produced,
}

/// One candidate impl for a deferred (or immediate) conversion call.
#[derive(Debug, Clone)]
pub(crate) struct ConversionCandidate {
    /// The candidate impl (cloned out of the registry).
    pub imp: TraitImpl,
    /// The dispatch symbol of the method the call compiles to (`into`'s for
    /// a direct impl, `from`'s for a bridged one — same runtime shape).
    pub symbol: Arc<str>,
    /// The call's result-type pattern: the method's return type with trait
    /// parameters replaced by the impl's arguments and any impl parameters
    /// already bound by the receiver substituted in. Params that only the
    /// result can bind (rare) remain and match at selection time.
    pub produced: Type,
    /// Which type the impl's target covers for dictionary recording.
    pub dispatch_receiver: DispatchReceiver,
    /// The trait name the user wrote, for diagnostics.
    pub trait_name: Arc<str>,
}

/// A conversion call awaiting result-type-directed impl selection.
#[derive(Debug)]
pub(crate) struct PendingMethodSelection {
    /// The [`ResolvedMethod::Pending`] marker on the call expression.
    pub id: u32,
    /// The receiver's (applied) type.
    pub receiver: Type,
    /// The call's result type — an inference variable the surrounding body
    /// pins (an annotation, an argument position).
    pub ret: Type,
    /// The candidate impls.
    pub candidates: Vec<ConversionCandidate>,
    /// The method name, for diagnostics.
    pub method_name: Arc<str>,
    /// Span of the call site.
    pub span: (u32, u32),
}

/// The outcome of resolving a conversion call at the call site.
pub(crate) enum ConversionResolution {
    /// Exactly one candidate: dispatch resolved now. The caller unifies the
    /// call's result with `produced`.
    Immediate {
        symbol: Arc<str>,
        produced: Type,
        dicts: Option<Dicts>,
    },
    /// Several candidates: selection deferred to the body's settle point.
    /// The caller records `Pending(id)` dispatch and returns `ret` as the
    /// call's type.
    Deferred { id: u32 },
}

impl Infer {
    /// The `Into` bridge candidates for a receiver: every
    /// `impl From<S> for T` where `S`'s head matches the receiver's. The
    /// produced type is the impl's target with any parameters bound by the
    /// receiver substituted in; an impl whose `From` argument doesn't
    /// actually match the receiver (head-equal but argument-different) is
    /// dropped here.
    pub(crate) fn into_bridge_candidates(&self, receiver: &Type) -> Vec<ConversionCandidate> {
        let Some(head) = trait_arg_head(receiver) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for imp in self
            .trait_registry
            .impls_with_first_arg(TRAIT_FROM_UUID, head)
        {
            let Some(source) = imp.trait_args.first() else {
                continue;
            };
            let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
            if !match_target(source, receiver, &mut subst) {
                continue;
            }
            let Some(target) = &imp.target else {
                continue;
            };
            let Some(symbol) = imp.methods.get("from").cloned() else {
                continue;
            };
            out.push(ConversionCandidate {
                imp: imp.clone(),
                symbol,
                produced: substitute_rigid_params(target, &subst),
                dispatch_receiver: DispatchReceiver::Produced,
                trait_name: Arc::from("Into"),
            });
        }
        out
    }

    /// Candidates from the direct impls of `trait_uuid` on the receiver's
    /// type that provide a zero-argument `method_name`: one per impl, the
    /// produced type derived from the trait method's return type under the
    /// impl's trait arguments (and any impl parameters the receiver binds).
    pub(crate) fn direct_method_candidates(
        &mut self,
        trait_uuid: uuid::Uuid,
        type_uuid: uuid::Uuid,
        receiver: &Type,
        method_name: &str,
    ) -> Vec<ConversionCandidate> {
        let Some(trait_def) = self.trait_registry.get_trait(trait_uuid).cloned() else {
            return Vec::new();
        };
        let Some(method) = trait_def
            .methods
            .iter()
            .find(|m| m.name.as_ref() == method_name && m.has_self)
        else {
            return Vec::new();
        };
        let receiver = self.apply(receiver);
        let mut out = Vec::new();
        let impls: Vec<TraitImpl> = self
            .trait_registry
            .impls_of(trait_uuid, type_uuid)
            .into_iter()
            .cloned()
            .collect();
        for imp in impls {
            let Some(symbol) = imp.methods.get(method_name).cloned() else {
                continue;
            };
            let mut subst: HashMap<Arc<str>, Type> = HashMap::new();
            if imp.is_generic
                && let Some(target) = &imp.target
                && !match_target(target, &receiver, &mut subst)
            {
                continue;
            }
            // The result pattern: trait parameters → this impl's arguments
            // (receiver-bound impl params substituted), then Self → the
            // receiver.
            let trait_arg_map: HashMap<Arc<str>, Type> = trait_def
                .type_params
                .iter()
                .cloned()
                .zip(
                    imp.trait_args
                        .iter()
                        .map(|a| substitute_rigid_params(a, &subst)),
                )
                .collect();
            let produced = super::check::substitute_named(&method.ret, &trait_arg_map);
            let produced = super::expr::substitute_self(
                &substitute_rigid_params(&produced, &subst),
                &receiver,
            );
            out.push(ConversionCandidate {
                imp,
                symbol,
                produced,
                dispatch_receiver: DispatchReceiver::Receiver,
                trait_name: Arc::clone(&trait_def.name),
            });
        }
        out
    }

    /// Resolve a conversion call given its candidates: immediately when one
    /// candidate exists (its produced type then pins the call's type), else
    /// deferred to the body's settle point with a fresh `Pending` marker.
    pub(crate) fn resolve_conversion(
        &mut self,
        candidates: Vec<ConversionCandidate>,
        receiver: &Type,
        method_name: &Arc<str>,
        span: (u32, u32),
    ) -> Option<ConversionResolution> {
        match candidates.len() {
            0 => None,
            1 => {
                let cand = &candidates[0];
                let dicts = self.candidate_dicts(cand, receiver, &cand.produced.clone(), span);
                Some(ConversionResolution::Immediate {
                    symbol: Arc::clone(&cand.symbol),
                    produced: cand.produced.clone(),
                    dicts,
                })
            }
            _ => {
                let id = self.next_method_selection;
                self.next_method_selection += 1;
                let ret = self.fresh();
                self.pending_method_selections.push(PendingMethodSelection {
                    id,
                    receiver: receiver.clone(),
                    ret,
                    candidates,
                    method_name: Arc::clone(method_name),
                    span,
                });
                Some(ConversionResolution::Deferred { id })
            }
        }
    }

    /// The result type of a deferred selection, for the caller to return as
    /// the call's type.
    pub(crate) fn selection_ret(&self, id: u32) -> Type {
        self.pending_method_selections
            .iter()
            .find(|s| s.id == id)
            .map_or(Type::Error, |s| s.ret.clone())
    }

    /// Record the hidden dictionaries a chosen candidate's call needs (a
    /// conditional impl's own bounds), against whichever type its target
    /// covers.
    fn candidate_dicts(
        &mut self,
        cand: &ConversionCandidate,
        receiver: &Type,
        produced: &Type,
        span: (u32, u32),
    ) -> Option<Dicts> {
        if !cand.imp.is_generic {
            return None;
        }
        let receiver_ty = match cand.dispatch_receiver {
            DispatchReceiver::Receiver => receiver.clone(),
            DispatchReceiver::Produced => produced.clone(),
        };
        self.record_conditional_impl_dicts(
            cand.imp.target.as_ref(),
            &cand.imp.bounds.clone(),
            &receiver_ty,
            &cand.trait_name.clone(),
            span,
        )
    }

    /// Discharge every deferred selection recorded since the last call:
    /// apply the result type (now as resolved as it will be), pick the one
    /// candidate whose produced type matches, and hand back the dispatch
    /// rewrites for [`finalize_method_selections`]. Any dictionary
    /// constraints the chosen impl introduces are recorded here and solved
    /// by the caller's subsequent `solve_dict_constraints`.
    pub(crate) fn solve_method_selections(
        &mut self,
        errors: &mut Vec<BoxedTypeError>,
    ) -> HashMap<u32, (Arc<str>, Option<Dicts>)> {
        let mut out = HashMap::new();
        for sel in std::mem::take(&mut self.pending_method_selections) {
            let ret = self.apply(&sel.ret);
            if matches!(ret, Type::Var(_)) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::CannotInfer {
                        hint: format!(
                            "conversion target of `{}` (add an annotation)",
                            sel.method_name
                        ),
                    },
                    sel.span,
                )));
                continue;
            }
            let matches: Vec<&ConversionCandidate> = sel
                .candidates
                .iter()
                .filter(|cand| {
                    let mut subst = HashMap::new();
                    match_target(&cand.produced, &ret, &mut subst)
                })
                .collect();
            match matches.as_slice() {
                [cand] => {
                    let cand = (*cand).clone();
                    // Bind the result so recorded expression types render
                    // the selected conversion, not a dangling variable.
                    let _ = self.unify(&sel.ret, &ret, sel.span);
                    let dicts = self.candidate_dicts(&cand, &sel.receiver, &ret, sel.span);
                    out.insert(sel.id, (cand.symbol, dicts));
                }
                [] => {
                    errors.push(Box::new(TypeError::new(
                        TypeErrorKind::BoundNotSatisfied {
                            ty: self.apply(&sel.receiver),
                            trait_name: sel
                                .candidates
                                .first()
                                .map_or_else(|| Arc::from("Into"), |c| Arc::clone(&c.trait_name)),
                        },
                        sel.span,
                    )));
                }
                _ => {
                    errors.push(Box::new(TypeError::new(
                        TypeErrorKind::AmbiguousMethod {
                            method: Arc::clone(&sel.method_name),
                            ty: self.apply(&sel.receiver),
                            candidates: matches
                                .iter()
                                .map(|c| Arc::from(format!("{}", c.produced)))
                                .collect(),
                        },
                        sel.span,
                    )));
                }
            }
        }
        out
    }
}

/// Rewrite every [`ResolvedMethod::Pending`] marker in `expr` to its
/// selected symbol and dictionary annotation. A marker missing from
/// `solved` (a reported selection error) stays pending; the module carries
/// errors and never compiles.
pub(crate) fn finalize_method_selections(
    expr: &mut crate::ast::Expr,
    solved: &HashMap<u32, (Arc<str>, Option<Dicts>)>,
) {
    walk_exprs_mut(expr, &mut |e| {
        if let crate::ast::ExprKind::MethodCall {
            resolved_method: Some(ResolvedMethod::Pending(id)),
            ..
        } = &e.kind
            && let Some((symbol, dicts)) = solved.get(id)
        {
            let symbol = Arc::clone(symbol);
            let dicts = dicts.clone();
            if let crate::ast::ExprKind::MethodCall {
                resolved_method, ..
            } = &mut e.kind
            {
                *resolved_method = Some(ResolvedMethod::Symbol(symbol));
            }
            if dicts.is_some() {
                e.dicts = dicts;
            }
        }
    });
}
