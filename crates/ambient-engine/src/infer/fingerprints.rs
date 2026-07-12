//! State-cell fingerprint recording and rendering.
//!
//! Cells remember the static type they were last written at — a
//! **fingerprint** of the writer's canonical type, threaded through the
//! perform by the compiler (see `ref/live-upgrade.md`, "Migration"). The
//! State ability's write-path methods declare trailing `String`
//! fingerprint parameters that call sites never spell:
//!
//! - the **checker** (this module, called from `lookup_dynamic_method`)
//!   hides those parameters from perform-site arity and records the
//!   instantiated cell types as a pending fingerprint group — resolved
//!   only once the enclosing body is fully inferred, exactly like
//!   trait-bound dictionaries. The methods' `make`/`migrate`/`f`
//!   parameters are real effect-polymorphic function types (`() -> S with
//!   E`), so ordinary argument unification constrains them and solves the
//!   cell type; this module never synthesizes a function shape;
//! - the **compiler** pushes each rendered fingerprint as a hidden
//!   trailing string argument at the perform site (before dictionaries),
//!   so the default implementations receive them as ordinary parameters
//!   and pass them to the `state_*` externs.
//!
//! The ability is recognized by its reserved uuid
//! ([`ambient_core::state::STATE_UUID`], the Exception-anchor precedent),
//! never by name; the methods within it by name, which is safe because a
//! rename desynchronizes arity loudly (the hidden-parameter carve-out
//! stops applying) rather than silently changing fingerprints.
//!
//! Rendering reuses [`CanonicalTypeRenderer`] — one renderer per
//! fingerprint, so variable numbering is fingerprint-local and the
//! rendering is byte-stable across compiles (the same property ability
//! signature hashes pin). A type that still mentions a rigid type
//! parameter of the enclosing item is an error: the fingerprint would
//! change meaning per instantiation, which dictionary-free compilation
//! cannot express.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::{CanonicalTypeRenderer, DynMethod};
use crate::ast::{Expr, ExprKind, Fingerprints, walk_exprs_mut};
use crate::types::{Type, TypeVarId};

use super::error::{BoxedTypeError, TypeError, TypeErrorKind, type_error};
use super::{Infer, InferResult};

/// One perform site's unrendered fingerprint types, waiting for the
/// enclosing body's inference to settle.
pub(crate) struct PendingFingerprint {
    /// The instantiated cell types to render, in hidden-parameter order.
    tys: Vec<Type>,
    /// The [`Fingerprints::Pending`] group on the perform expression.
    group: u32,
    /// Span of the perform, for diagnostics.
    span: (u32, u32),
}

/// How many trailing parameters of a State method are compiler-supplied
/// fingerprints. Zero for read paths (`get`) and unknown methods.
pub(crate) fn hidden_fingerprint_params(method_name: &str) -> usize {
    match method_name {
        "init" | "set" | "update" => 1,
        "init_versioned" => 2,
        _ => 0,
    }
}

/// The instantiation of the method's `idx`-th type parameter at this
/// perform site. Errors loudly if the declaration's shape drifted from
/// the spec this module encodes (a platform/engine version skew).
fn quantified_ty(
    method: &DynMethod,
    subst: &HashMap<TypeVarId, Type>,
    idx: usize,
    span: (u32, u32),
) -> InferResult<Type> {
    method
        .quantified
        .get(idx)
        .and_then(|var| subst.get(var))
        .cloned()
        .ok_or_else(|| {
            type_error(
                TypeErrorKind::InvalidDeclaration {
                    message: format!(
                        "State::{} does not declare the type parameters its \
                         fingerprint contract requires (platform declaration drift)",
                        method.name
                    ),
                },
                span,
            )
        })
}

impl Infer {
    /// Record the fingerprint obligations of a State perform site: a
    /// pending fingerprint group on the perform expression, holding the
    /// instantiated cell types that render into the hidden trailing
    /// strings. Call only for methods with [`hidden_fingerprint_params`]
    /// > 0, after instantiation.
    ///
    /// The retyped State signatures carry real [`Type::Function`]
    /// parameters (`make: () -> S with E`, `f: (S) -> S with E`, ...), so
    /// `lookup_dynamic_method`'s ordinary argument unification already
    /// constrains `make`/`migrate`/`f` to their argument shapes and solves
    /// the cell type — this method never synthesizes a function shape of
    /// its own. It only names which instantiated type parameters render
    /// into the fingerprint strings, in hidden-parameter order.
    pub(crate) fn record_state_fingerprints(
        &mut self,
        method: &DynMethod,
        subst: &HashMap<TypeVarId, Type>,
        fingerprints: &mut Option<Fingerprints>,
        span: (u32, u32),
    ) -> InferResult<()> {
        let tys = match method.name.as_ref() {
            // init<S, E!>(name, make: () -> S with E, fingerprint):
            //   fingerprint = S
            // set<S>(name, value: S, fingerprint): fingerprint = S
            // update<S, E!>(name, f: (S) -> S with E, fingerprint):
            //   fingerprint = S
            "init" | "set" | "update" => vec![quantified_ty(method, subst, 0, span)?],
            // init_versioned<S, O, E!, F!>(name, make: () -> S with E,
            //   migrate: (O) -> S with F, old_fingerprint, new_fingerprint):
            //   old_fingerprint = O (quantified[1]), new_fingerprint = S
            //   (quantified[0]).
            "init_versioned" => vec![
                quantified_ty(method, subst, 1, span)?,
                quantified_ty(method, subst, 0, span)?,
            ],
            _ => return Ok(()),
        };

        let group = self.next_fingerprint_group;
        self.next_fingerprint_group += 1;
        self.pending_fingerprints
            .push(PendingFingerprint { tys, group, span });
        *fingerprints = Some(Fingerprints::Pending(group));
        Ok(())
    }

    /// Render every fingerprint recorded since the last call. Runs after
    /// an item body is fully inferred, so the cell types are as resolved
    /// as they will ever be; a type still mentioning a rigid type
    /// parameter is an error (a fingerprint must mean one type).
    pub(crate) fn solve_fingerprints(
        &mut self,
        errors: &mut Vec<BoxedTypeError>,
    ) -> HashMap<u32, Vec<Arc<str>>> {
        let pending = std::mem::take(&mut self.pending_fingerprints);
        let mut solved = HashMap::new();
        for fingerprint in pending {
            let mut strings: Vec<Arc<str>> = Vec::with_capacity(fingerprint.tys.len());
            for ty in &fingerprint.tys {
                let ty = self.apply(ty);
                if ty.mentions_param() {
                    errors.push(Box::new(TypeError::new(
                        TypeErrorKind::GenericStateWrite { ty },
                        fingerprint.span,
                    )));
                    // Keep the group's arity intact for the compiler;
                    // Error-typed modules never compile anyway.
                    strings.push(Arc::from("<generic>"));
                    continue;
                }
                // One renderer per fingerprint: variable numbering is
                // fingerprint-local, matching the byte-stable convention
                // ability signatures hash under.
                let mut renderer = CanonicalTypeRenderer::new();
                strings.push(Arc::from(renderer.render(&ty)));
            }
            solved.insert(fingerprint.group, strings);
        }
        solved
    }
}

/// Rewrite every [`Fingerprints::Pending`] annotation in `expr` to its
/// rendered strings. A group missing from `solved` (a checker bug) is
/// left pending; the compiler reports it as an internal error rather than
/// miscompiling.
pub(crate) fn finalize_fingerprints(expr: &mut Expr, solved: &HashMap<u32, Vec<Arc<str>>>) {
    walk_exprs_mut(expr, &mut |e| {
        if let ExprKind::Perform(call) = &mut e.kind
            && let Some(Fingerprints::Pending(group)) = &call.fingerprints
            && let Some(rendered) = solved.get(group)
        {
            call.fingerprints = Some(Fingerprints::Resolved(rendered.clone()));
        }
    });
}
