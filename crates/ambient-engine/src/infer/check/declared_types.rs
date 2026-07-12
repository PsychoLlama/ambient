//! Resolve-or-error for written type annotations, and the module-wide
//! declared-type validation sweep (Phase 1, last step): undefined-type
//! reporting, `Type::Error` sanitization, and the public-signature
//! annotation requirement. Split from `locals.rs` (per-file line budgets).

use std::sync::Arc;

use crate::types::Type;

use crate::infer::Infer;
use crate::infer::error::{BoxedTypeError, TypeError, TypeErrorKind};

// ─────────────────────────────────────────────────────────────────────────────
// Resolve-or-error for type annotations
// ─────────────────────────────────────────────────────────────────────────────

/// Whether `name` denotes a type that exists in the module's world: a rigid
/// parameter in scope (`extra_known`), the `Self` placeholder, a registered
/// type alias/struct (including opaque generic heads like a prelude `List`),
/// or a registered enum.
/// The predicate a written type annotation must satisfy to not be "undefined".
///
/// The four primitives are **not** a special case here: they are ordinary
/// prelude imports registered as type aliases (`String`, `Number`, …), so a
/// registry-backed module finds them via `get_type_alias`. Deliberately *not*
/// falling back to `Primitive::from_name` keeps this predicate in lockstep with
/// [`Infer::expand_named_alias`], which no longer resolves primitives by name
/// without a registry. A stale primitive branch here would declare a bare
/// `String` "known" while resolution left it an opaque, uuid-less `Named` —
/// leaking past resolve-or-error into unification and signature hashes instead
/// of being sanitized to `Type::Error`. Registry-less checks (which never seed
/// the primitive aliases) therefore correctly report a bare `String`
/// annotation as `UndefinedTypeName`.
///
/// `Type::Param` (a resolved rigid parameter) never reaches the checks that
/// call this — it isn't a `Named` — so a body's own type parameters are never
/// flagged even without appearing in `extra_known`; the set only carries the
/// *unresolved* parameter names a raw-AST sweep sees.
fn is_known_type_name(
    infer: &Infer,
    name: &str,
    extra_known: &std::collections::HashSet<Arc<str>>,
) -> bool {
    extra_known.contains(name)
        || name == "Self"
        || infer.get_type_alias(name).is_some()
        || infer.enum_registry.get(name).is_some()
}
/// Report `UndefinedTypeName` for every unknown nominal head name in a
/// *written* (unresolved, raw-AST) type annotation, recursing into composite
/// children and type arguments.
///
/// Checks the **head** name, so a generic user type (`Pair<A, B>`) stays
/// valid while `Nope<A>` is flagged — an undefined head makes its arguments
/// moot. Reporting-only (does not rewrite): the declared-types sweep uses it
/// on raw AST types, which sidesteps `resolve_holes` (and its alias
/// expansion, which would loop on a self-referential struct).
fn report_undefined_types(
    infer: &Infer,
    ty: &Type,
    span: (u32, u32),
    extra_known: &std::collections::HashSet<Arc<str>>,
    errors: &mut Vec<BoxedTypeError>,
) {
    match ty {
        // `Handler<A, R>`: a dedicated syntactic node whose `A` is an
        // *ability* reference resolved through the ability namespace — not a
        // type, so it must not be flagged here. Only the optional answer type
        // (`R`) is a real type annotation to check. (Any bad ability
        // reference is reported when `resolve_holes` resolves it.)
        Type::HandlerAnnotation(h) => {
            if let Some(answer) = &h.answer {
                report_undefined_types(infer, answer, span, extra_known, errors);
            }
        }
        Type::Named(n) => {
            // A uuid-carrying head is already resolved — a qualified
            // spelling the resolve pass canonicalized to identity (e.g.
            // `core::collections::List<T>` → `Named("List", …, uuid)`).
            // Its bare name needn't be in scope; only its arguments are
            // still written annotations to check.
            if n.uuid.is_none() && !is_known_type_name(infer, &n.name, extra_known) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::UndefinedTypeName {
                        name: Arc::clone(&n.name),
                    },
                    span,
                )));
                return;
            }
            for arg in &n.args {
                report_undefined_types(infer, arg, span, extra_known, errors);
            }
        }
        Type::Tuple(elems) => {
            for e in elems {
                report_undefined_types(infer, e, span, extra_known, errors);
            }
        }
        Type::Record(rec) => {
            for (_, t) in &rec.fields {
                report_undefined_types(infer, t, span, extra_known, errors);
            }
        }
        Type::Function(f) => {
            for p in &f.params {
                report_undefined_types(infer, p, span, extra_known, errors);
            }
            report_undefined_types(infer, &f.ret, span, extra_known, errors);
        }
        Type::AbilityValue(av) => {
            report_undefined_types(infer, &av.result, span, extra_known, errors);
        }
        Type::Forall(fa) => report_undefined_types(infer, &fa.body, span, extra_known, errors),
        // Leaves, and already-resolved forms (`Nominal`, `Param`, `Var`,
        // primitives, `Unit`, ...): nothing to flag. `Nominal` inners belong
        // to some *other* declaration, already checked at its own site.
        _ => {}
    }
}
/// The rigid-parameter name set for a declaration's own type parameters.
fn type_param_set(params: &[crate::ast::TypeParam]) -> std::collections::HashSet<Arc<str>> {
    params.iter().map(|tp| Arc::clone(&tp.name)).collect()
}
/// Rewrite every unresolved nominal reference in a *resolved* type to
/// `Type::Error`, so an undefined type never leaks into unification or a
/// signature hash as an opaque `Named`. Non-reporting: the declared-types
/// sweep already reports these (see [`report_undefined_types`]); this just
/// keeps the checked/hashed type clean so no cascade or leak follows.
/// `Type::Error` unifies away, so downstream uses see no secondary error.
fn error_undefined_types(infer: &Infer, ty: &Type) -> Type {
    let empty = std::collections::HashSet::new();
    match ty {
        Type::Named(n) if n.uuid.is_none() && !is_known_type_name(infer, &n.name, &empty) => {
            Type::Error
        }
        Type::Named(n) => Type::Named(
            n.map_args(
                n.args
                    .iter()
                    .map(|a| error_undefined_types(infer, a))
                    .collect(),
            ),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| error_undefined_types(infer, e))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), error_undefined_types(infer, t)))
                .collect(),
        )),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| error_undefined_types(infer, p))
                .collect(),
            error_undefined_types(infer, &f.ret),
            f.abilities.clone(),
        ),
        Type::AbilityValue(av) => {
            Type::ability_value(error_undefined_types(infer, &av.result), av.ability.clone())
        }
        Type::Forall(fa) => Type::Forall(crate::types::ForallType::with_abilities(
            fa.vars.clone(),
            fa.ability_vars.clone(),
            error_undefined_types(infer, &fa.body),
        )),
        // `Nominal` inner is another declaration's already-resolved body;
        // leave it (and every leaf) untouched.
        _ => ty.clone(),
    }
}
/// Resolve holes/aliases in a written annotation and rewrite any leftover
/// undefined nominal reference to `Type::Error`. The value-side counterpart
/// to reporting: every signature and body annotation runs through this so the
/// *checked* type never carries an opaque `Named`. Reporting is done once, by
/// the declared-types sweep, keeping diagnostics free of duplicates.
pub(super) fn resolve_erroring(infer: &mut Infer, ty: &Type) -> Type {
    let resolved = infer.resolve_holes(ty);
    error_undefined_types(infer, &resolved)
}
/// Resolve a *body-local* annotation — a `let` binding or a lambda parameter
/// type — reporting any undefined type name and rewriting it to
/// `Type::Error`. These annotations are the one kind the declared-types sweep
/// can't reach (they live inside expression bodies), so this both reports (to
/// `pending_errors`, drained module-wide) and rewrites, at their sole
/// resolution site in [`infer::expr`](crate::infer::expr). Rigid type parameters in
/// scope resolve to `Type::Param` first, so a body's own `T` is never flagged.
pub(in crate::infer) fn resolve_body_annotation(
    infer: &mut Infer,
    ty: &Type,
    span: (u32, u32),
) -> Type {
    let resolved = infer.resolve_holes(ty);
    let no_extra = std::collections::HashSet::new();
    let mut reported = Vec::new();
    report_undefined_types(infer, &resolved, span, &no_extra, &mut reported);
    infer.pending_errors.extend(reported);
    error_undefined_types(infer, &resolved)
}
/// Report undefined type names across every local declaration's written type
/// annotations (Phase 1, after all local types are registered so self- and
/// mutually-recursive names already resolve).
///
/// The single reporting authority for undefined types in declared
/// signatures: it walks raw AST types (foreign items untouched — only the
/// current module's `items`), so the scheme builders and body checkers can
/// rewrite to `Type::Error` without also reporting, keeping each undefined
/// type exactly one diagnostic. In-body `let`/lambda annotations are the one
/// exception — reported inline in `infer::expr`, as they never reach here.
pub(super) fn check_declared_types(
    infer: &Infer,
    module: &crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) {
    let empty = std::collections::HashSet::new();
    for item in &module.items {
        let span = (item.span.start, item.span.end);
        match &item.kind {
            crate::ast::ItemKind::Function(func) => {
                let known = type_param_set(&func.type_params);
                for p in &func.params {
                    if let Some(ty) = &p.ty {
                        let s = (p.span.start, p.span.end);
                        report_undefined_types(infer, ty, s, &known, errors);
                    }
                }
                if let Some(ret) = &func.ret_ty {
                    report_undefined_types(infer, ret, span, &known, errors);
                }
                enforce_public_signature(func, span, errors);
            }
            crate::ast::ItemKind::ExternFn(def) => {
                let known = type_param_set(&def.type_params);
                for p in &def.params {
                    if let Some(ty) = &p.ty {
                        let s = (p.span.start, p.span.end);
                        report_undefined_types(infer, ty, s, &known, errors);
                    }
                }
                report_undefined_types(infer, &def.ret_ty, span, &known, errors);
            }
            crate::ast::ItemKind::Const(c) => {
                if let Some(ty) = &c.ty {
                    report_undefined_types(infer, ty, span, &empty, errors);
                }
            }
            crate::ast::ItemKind::Struct(s) => {
                let known = type_param_set(&s.type_params);
                let s_span = (s.name_span.start, s.name_span.end);
                for field_ty in struct_field_types(&s.ty) {
                    report_undefined_types(infer, field_ty, s_span, &known, errors);
                }
            }
            crate::ast::ItemKind::Enum(e) => {
                let known = type_param_set(&e.type_params);
                for v in &e.variants {
                    if let Some(payload) = &v.payload {
                        let s = (v.span.start, v.span.end);
                        report_undefined_types(infer, payload, s, &known, errors);
                    }
                }
            }
            crate::ast::ItemKind::Trait(t) => {
                for m in &t.methods {
                    let known = type_param_set(&m.type_params);
                    let s = (m.span.start, m.span.end);
                    for (_, pty) in &m.params {
                        report_undefined_types(infer, pty, s, &known, errors);
                    }
                    report_undefined_types(infer, &m.ret_ty, s, &known, errors);
                }
            }
            crate::ast::ItemKind::Ability(a) => {
                for m in &a.methods {
                    let known = type_param_set(&m.type_params);
                    let s = (m.span.start, m.span.end);
                    for p in &m.params {
                        report_undefined_types(infer, p.declared_ty(), s, &known, errors);
                    }
                    report_undefined_types(infer, &m.ret_ty, s, &known, errors);
                }
            }
            crate::ast::ItemKind::Impl(imp) => {
                // The impl target (`impl Strng`) is validated elsewhere
                // (invalid-target / structural-type errors); only the method
                // signatures are swept here.
                let impl_known = type_param_set(&imp.type_params);
                for m in &imp.methods {
                    let method_known: std::collections::HashSet<Arc<str>> = impl_known
                        .iter()
                        .cloned()
                        .chain(m.type_params.iter().map(|tp| Arc::clone(&tp.name)))
                        .collect();
                    for p in &m.params {
                        if let Some(ty) = &p.ty {
                            let s = (p.span.start, p.span.end);
                            report_undefined_types(infer, ty, s, &method_known, errors);
                        }
                    }
                    if let Some(ret) = &m.ret_ty {
                        let s = (m.span.start, m.span.end);
                        report_undefined_types(infer, ret, s, &method_known, errors);
                    }
                }
            }
            _ => {}
        }
    }
}
/// Require a public function to declare its full signature: every parameter
/// type and the return type.
///
/// A `pub fn` signature is the cross-module contract: importing modules
/// rebuild the callable scheme from the written annotations alone
/// (`get_symbol_scheme` in [`super::foreign`]), so an omitted type would hand
/// foreign callers a free variable no body constraint can ever reach — the
/// cross-module edition of the unsoundness body↔scheme sharing closes within
/// a module (see `check_function_body`). Private functions stay inferable.
fn enforce_public_signature(
    func: &crate::ast::FunctionDef,
    span: (u32, u32),
    errors: &mut Vec<BoxedTypeError>,
) {
    if !func.is_public {
        return;
    }
    for p in &func.params {
        if p.ty.is_none() {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::PublicFnMissingAnnotation {
                    func: Arc::clone(&func.name),
                    position: Arc::from(format!("the type of parameter `{}`", p.name)),
                },
                (p.span.start, p.span.end),
            )));
        }
    }
    if func.ret_ty.is_none() {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::PublicFnMissingAnnotation {
                func: Arc::clone(&func.name),
                position: Arc::from("a return type"),
            },
            span,
        )));
    }
}
/// The field types of a struct declaration's stored type. A declared struct
/// is a `Type::Nominal` wrapping a `Record` (a non-`unique` struct is the
/// bare `Record`); either way its fields are the written annotations.
fn struct_field_types(ty: &Type) -> Vec<&Type> {
    let record = match ty {
        Type::Nominal(n) => &*n.inner,
        other => other,
    };
    match record {
        Type::Record(rec) => rec.fields.iter().map(|(_, t)| t).collect(),
        _ => Vec::new(),
    }
}
