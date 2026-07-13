//! Dictionary annotations for bounded generics.
//!
//! A generic function with trait bounds (`fn contains<T: Eq>(...)`) compiles
//! with hidden trailing *dictionary parameters* — one per bound, each a tuple
//! of function values, the trait's methods in [`dictionary
//! order`](crate::types::TraitDef::dictionary_order). Nothing about a
//! function's *type* mentions dictionaries; they are a calling-convention
//! fact the checker resolves and the compiler implements:
//!
//! - A **call site** must supply each dictionary. The checker solves each
//!   bound against the instantiated type argument and annotates the callee
//!   reference with a [`DictSource`] per bound: either a concrete impl's
//!   method symbols (the compiler links them to content hashes exactly like
//!   a direct call — hash-pinned dispatch, no runtime registry) or a
//!   forwarded dictionary parameter of the *enclosing* bounded function.
//! - A **bound-method call** in the generic body (`x.eq(y)` where `x: T`,
//!   `T: Eq`) compiles as a tuple access into the dictionary parameter plus
//!   an indirect call — see [`ResolvedMethod::DictSlot`].
//!
//! Annotations are attached to expressions ([`super::Expr::dicts`]) in two
//! phases: inference records a pending constraint group (types may still be
//! unresolved inference variables mid-body), and after the enclosing body is
//! fully checked the solved sources replace the pending marker (see
//! `Infer::solve_dict_constraints`).

use std::sync::Arc;

/// Where one dictionary argument comes from at one call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictSource {
    /// Build the dictionary from a concrete impl: a tuple of function
    /// references, one per trait method in dictionary order. Each symbol
    /// resolves through the ordinary name→hash table, so the call site's
    /// content hash pins the exact impl methods it dispatches to.
    Impl {
        /// The impl's method symbols in dictionary order.
        symbols: Vec<Arc<str>>,
    },
    /// Forward the enclosing function's own dictionary parameter (the
    /// caller is itself generic over this bound). The index counts the
    /// enclosing function's bounds in declaration order; the compiler maps
    /// it to the hidden trailing parameter slot.
    Param {
        /// Index into the enclosing function's dictionary parameters.
        dict_index: usize,
    },
    /// Build the dictionary from a *conditional* (generic) impl such as
    /// `impl<T: Eq> Eq for Pair<T>`. The impl's own methods take hidden
    /// trailing dictionaries (one per the impl's bounds); solving the outer
    /// bound derived those inner dictionaries ([`inner`], one per impl bound
    /// in dictionary order). Each dictionary slot is therefore a *closure*
    /// over the inner dictionaries that, when invoked with the method's
    /// value arguments, calls the impl method with those arguments plus the
    /// captured inner dictionaries. The compiler synthesizes one closure per
    /// method and assembles them into the dictionary tuple.
    Generic {
        /// The impl's methods in dictionary order — the same order the
        /// trait's [`dictionary_order`](crate::types::TraitDef::dictionary_order)
        /// yields, so the tuple slots line up with bound-method dispatch.
        methods: Vec<GenericDictMethod>,
        /// The inner dictionaries the impl's own bounds resolve to, in the
        /// impl's [`dict_params`](crate::ast::dict_params) order. Every
        /// method closure captures all of them and forwards them as the
        /// method's hidden trailing dictionary arguments.
        inner: Vec<DictSource>,
    },
}

/// One method of a conditional impl's dictionary (see
/// [`DictSource::Generic`]): the impl-method symbol to call, how many value
/// arguments the dictionary slot forwards to it (receiver included), and how
/// many of the method's *own* bound dictionaries the slot forwards after the
/// impl's inner dictionaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericDictMethod {
    /// The impl method's canonical dispatch symbol
    /// ([`crate::types::impl_method_symbol`]); resolved through the ordinary
    /// name→hash table, so the call site's content hash pins the exact impl.
    pub symbol: Arc<str>,
    /// The number of value arguments the slot closure forwards to the impl
    /// method — `self` (if any) plus the method's declared parameters. The
    /// inner dictionaries follow these, filling the impl block's hidden
    /// trailing dictionary parameters.
    pub arity: usize,
    /// The number of the method's *own* bound dictionaries (`fn m<U: Eq>`),
    /// derived from the method's [`method_bounds`](crate::types::TraitMethodDef::method_bounds)
    /// — itself the single-authority [`dict_params`](crate::ast::dict_params)
    /// list. The bound-method call site pushes these as extra runtime
    /// arguments after the value arguments; the slot closure forwards them
    /// after the captured inner dictionaries, matching the impl method's
    /// `impl ++ method` trailing-dictionary layout.
    pub method_dict_count: usize,
}

/// The dictionary annotation on an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dicts {
    /// Inference recorded a constraint group here; solving hasn't run yet.
    /// The compiler treats a surviving `Pending` as an internal error — a
    /// checked module has only `Resolved` annotations.
    Pending(u32),
    /// The solved dictionary sources, one per bound of the instantiated
    /// scheme, in the scheme's bound order.
    Resolved(Vec<DictSource>),
}

/// The `State` fingerprint annotation on a perform expression (see
/// `ambient_core::state` and `ref/live-upgrade.md`, "Migration").
///
/// The State ability's write-path methods declare trailing `String`
/// fingerprint parameters that call sites never spell. The checker hides
/// them from perform-site arity and records the canonical rendering of the
/// instantiated cell type here (pending until the enclosing body is fully
/// inferred, exactly like [`Dicts`]); the compiler pushes each rendering
/// as a hidden trailing string argument, before any dictionaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fingerprints {
    /// Inference recorded a fingerprint group here; rendering hasn't run
    /// yet. The compiler treats a surviving `Pending` as an internal error.
    Pending(u32),
    /// The rendered canonical fingerprints, one per hidden parameter, in
    /// declaration order.
    Resolved(Vec<Arc<str>>),
}

/// How a method call or overloaded operator dispatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedMethod {
    /// A content-addressed function symbol (concrete receiver): the
    /// compiler resolves it through the name→hash table like any call.
    Symbol(Arc<str>),
    /// A bound-method call on a rigid type parameter: load the enclosing
    /// function's `dict_index`-th dictionary parameter, take tuple slot
    /// `slot`, call it with the receiver and arguments.
    DictSlot {
        /// Index into the enclosing function's dictionary parameters.
        dict_index: usize,
        /// Tuple slot within that dictionary (the method's position in
        /// dictionary order).
        slot: usize,
    },
}

impl ResolvedMethod {
    /// The content-addressed symbol, if this is a symbol dispatch.
    #[must_use]
    pub fn as_symbol(&self) -> Option<&Arc<str>> {
        match self {
            Self::Symbol(s) => Some(s),
            Self::DictSlot { .. } => None,
        }
    }
}

use super::{Expr, ExprKind};

/// Visit `expr` and every expression nested inside it, pre-order.
///
/// The checker uses this to finalize dictionary annotations after a body is
/// fully inferred; it deliberately lives next to the AST so a new
/// [`ExprKind`] variant fails to compile here until its children are listed.
pub fn walk_exprs_mut(expr: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    f(expr);
    match &mut expr.kind {
        ExprKind::Unit
        | ExprKind::Bool(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Local(_)
        | ExprKind::Name(_) => {}
        ExprKind::Tuple(elems) | ExprKind::List(elems) => {
            for e in elems {
                walk_exprs_mut(e, f);
            }
        }
        ExprKind::TupleIndex(e, _)
        | ExprKind::RecordField(e, _)
        | ExprKind::Unary(_, e)
        | ExprKind::Resume(e) => walk_exprs_mut(e, f),
        ExprKind::Record(fields) | ExprKind::TypedRecord { fields, .. } => {
            for (_, e) in fields {
                walk_exprs_mut(e, f);
            }
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            walk_exprs_mut(receiver, f);
            for a in args {
                walk_exprs_mut(a, f);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            walk_exprs_mut(left, f);
            walk_exprs_mut(right, f);
        }
        ExprKind::If(cond, then_br, else_br) => {
            walk_exprs_mut(cond, f);
            walk_exprs_mut(then_br, f);
            if let Some(e) = else_br {
                walk_exprs_mut(e, f);
            }
        }
        ExprKind::Match(scrutinee, arms) => {
            walk_exprs_mut(scrutinee, f);
            for arm in arms {
                walk_exprs_mut(&mut arm.body, f);
            }
        }
        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                match &mut stmt.kind {
                    super::StmtKind::Let(binding) => walk_exprs_mut(&mut binding.init, f),
                    super::StmtKind::Expr(e) => walk_exprs_mut(e, f),
                    super::StmtKind::Const(c) => walk_exprs_mut(&mut c.value, f),
                    super::StmtKind::Use(_) => {}
                }
            }
            if let Some(e) = result {
                walk_exprs_mut(e, f);
            }
        }
        ExprKind::Lambda(lambda) => walk_exprs_mut(&mut lambda.body, f),
        ExprKind::Call(callee, args) => {
            walk_exprs_mut(callee, f);
            for a in args {
                walk_exprs_mut(a, f);
            }
        }
        ExprKind::Perform(call) => {
            for a in &mut call.args {
                walk_exprs_mut(a, f);
            }
        }
        ExprKind::Handle(handle) => {
            for handler in &mut handle.handlers {
                walk_exprs_mut(handler, f);
            }
            walk_exprs_mut(&mut handle.body, f);
            if let Some(else_clause) = &mut handle.else_clause {
                walk_exprs_mut(else_clause, f);
            }
        }
        ExprKind::HandlerLiteral(lit) => {
            for arm in &mut lit.methods {
                walk_exprs_mut(&mut arm.body, f);
            }
        }
        ExprKind::Sandbox(sandbox) => walk_exprs_mut(&mut sandbox.body, f),
    }
}
