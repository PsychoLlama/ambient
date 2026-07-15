//! Structural AST hashing and canonical type rendering.
//!
//! Two jobs, both deterministic and *span-free* (so reformatting or
//! comment edits never move a hash):
//!
//! - [`render_type`] renders a [`Type`] to a canonical string. Unlike
//!   [`CanonicalTypeRenderer`](crate::ability_resolver::CanonicalTypeRenderer),
//!   this one is *total* over the post-resolve surface AST — it accepts
//!   `Type::Param`, `Type::Hole`, and `Type::HandlerAnnotation` without
//!   panicking, because signatures reach the interface before the checker
//!   normalizes them. It is a fresh, self-contained renderer rather than a
//!   reuse of the ability-signature one precisely so it can be lenient here
//!   without weakening the strict tripwires the ability path relies on.
//! - [`hash_body`] and [`module_ast_hash`] structurally hash expression
//!   subtrees (impl-method / ability-default bodies) and whole modules.
//!
//! # Binding-id normalization
//!
//! `BindingId`s are assigned module-globally by the parser, so an edit to
//! one item renumbers the locals of every later item. Hashing the raw ids
//! would make an impl-method body hash depend on unrelated private edits —
//! silently over-invalidating. Every hash unit therefore renumbers the
//! binding ids it sees to a local sequence on first occurrence
//! ([`BindingNorm`]), so a body hash depends only on that body's shape.
//! Binding-site *names* are likewise excluded (renaming a local is not a
//! content change), matching the content-addressing rule; field, variant,
//! and item names — which select data — are kept.

// Length prefixes are u32 by design (mirrors `object`): no AST subtree has
// 2^32 nodes, and fixed-width prefixes keep the hashing canonical.
#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::ast::{
    AbilityCall, BinaryOp, BindingId, Expr, ExprKind, HandleExpr, HandlerLiteralExpr, Item,
    ItemKind, Lambda, Literal, MatchArm, Param, Pattern, PatternKind, QualifiedName, SandboxExpr,
    Stmt, StmtKind, UnaryOp, UseDef,
};
use crate::fqn::NameKey;
use crate::types::{AbilityId, AbilitySet, Type};

/// Domain separator for body (expression subtree) hashes.
const BODY_DOMAIN: &[u8] = b"ambient/interface/body/v1";
/// Domain separator for whole-module resolved-AST hashes.
const MODULE_DOMAIN: &[u8] = b"ambient/interface/module/v1";
/// Domain separator for a single item's structural hash.
const ITEM_DOMAIN: &[u8] = b"ambient/interface/item/v1";

// ─────────────────────────────────────────────────────────────────────────────
// Canonical type rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Render a type to its canonical, deterministic string form.
///
/// Total over every [`Type`] variant, including the ones that only exist
/// pre-check (`Param`, `Hole`, `HandlerAnnotation`). Resolved nominal heads
/// render by uuid (rename-stable); unresolved heads and type parameters
/// fall back to their spelled name (byte-stable, not rename-stable — the
/// documented limit, matching the ability-signature renderer).
#[must_use]
pub fn render_type(ty: &Type) -> String {
    let mut s = String::new();
    write_type(&mut s, ty);
    s
}

fn write_type(out: &mut String, ty: &Type) {
    match ty {
        Type::Unit => out.push_str("()"),
        Type::Never => out.push('!'),
        Type::Error => out.push_str("<error>"),
        Type::Hole => out.push('_'),
        Type::Var(id) => {
            let _ = write!(out, "var{id}");
        }
        Type::Param(name) => {
            let _ = write!(out, "param:{name}");
        }
        Type::Tuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_type(out, e);
            }
            out.push(')');
        }
        Type::Record(rec) => {
            // Fields are already sorted by name (`RecordType::new`).
            out.push('{');
            for (i, (name, fty)) in rec.fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
                out.push_str(": ");
                write_type(out, fty);
            }
            out.push('}');
        }
        Type::Function(f) => {
            out.push_str("fn(");
            for (i, p) in f.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_type(out, p);
            }
            out.push_str(") -> ");
            write_type(out, &f.ret);
            let abilities = render_ability_set(&f.abilities);
            if !abilities.is_empty() {
                out.push_str(" with ");
                out.push_str(&abilities);
            }
        }
        Type::Named(n) => {
            match n.uuid {
                Some(uuid) => {
                    let _ = write!(out, "named:{uuid}");
                }
                None => {
                    let _ = write!(out, "named:{}", n.name);
                }
            }
            if !n.args.is_empty() {
                out.push('<');
                for (i, a) in n.args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_type(out, a);
                }
                out.push('>');
            }
        }
        Type::Nominal(n) => {
            let _ = write!(out, "nominal:{}:", n.uuid);
            if n.is_extern {
                out.push_str("extern:");
            }
            write_type(out, &n.inner);
        }
        Type::Handler(h) => {
            let _ = write!(out, "handler<{}>", h.ability.to_hex());
        }
        Type::HandlerAnnotation(h) => {
            let _ = write!(out, "handler-annot<{}", h.ability);
            if let Some(answer) = &h.answer {
                out.push_str(", ");
                write_type(out, answer);
            }
            out.push('>');
        }
        Type::Forall(forall) => {
            out.push_str("forall(");
            write_type(out, &forall.body);
            out.push(')');
        }
    }
}

/// Render an ability set (used inside function types) deterministically.
/// Concrete ids sort by hex; ability variables number `e{id}`; unresolved
/// names sort lexically.
#[must_use]
pub fn render_ability_set(set: &AbilitySet) -> String {
    match set {
        AbilitySet::Empty => String::new(),
        AbilitySet::Concrete(ids) => {
            let mut ids: Vec<String> = ids.iter().map(AbilityId::to_hex).collect();
            ids.sort_unstable();
            ids.join(", ")
        }
        AbilitySet::Var(id) => format!("e{id}"),
        AbilitySet::Row { concrete, tail } => {
            let mut ids: Vec<String> = concrete.iter().map(AbilityId::to_hex).collect();
            ids.sort_unstable();
            ids.push(format!("e{tail}"));
            ids.join(", ")
        }
        AbilitySet::Unresolved(names) => {
            let mut names: Vec<String> = names.iter().map(|n| format!("?{n}")).collect();
            names.sort_unstable();
            names.join(", ")
        }
    }
}

/// Render a reference to a named item by its resolved identity when the
/// resolve pass canonicalized it, else its spelled qualified form.
#[must_use]
pub fn render_name(qn: &QualifiedName) -> String {
    match qn.resolution_key() {
        NameKey::Item(fqn) => format!("@{fqn}"),
        NameKey::Bare(s) => format!("~{s}"),
    }
}

/// Render a trait reference: the name plus any trait type arguments
/// (`From<String>` → `@core::convert::From<String>`). An argument-less
/// reference renders byte-identically to [`render_name`], so pre-existing
/// shapes are unchanged.
pub fn render_trait_ref(tr: &crate::ast::TraitRef) -> String {
    let mut s = render_name(&tr.name);
    if !tr.args.is_empty() {
        s.push('<');
        for (i, arg) in tr.args.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&render_type(arg));
        }
        s.push('>');
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Structural body / item hashing
// ─────────────────────────────────────────────────────────────────────────────

/// Renumbers module-global binding ids to a hash-unit-local sequence on
/// first occurrence, so a body hash never depends on how many bindings were
/// declared before it in the module.
#[derive(Default)]
struct BindingNorm {
    map: HashMap<BindingId, u32>,
    next: u32,
}

impl BindingNorm {
    fn get(&mut self, id: BindingId) -> u32 {
        if let Some(&n) = self.map.get(&id) {
            return n;
        }
        let n = self.next;
        self.next += 1;
        self.map.insert(id, n);
        n
    }
}

/// A byte sink that structural hashing writes tagged, length-prefixed node
/// data into. Hashing the accumulated bytes yields the content hash.
struct Sink {
    buf: Vec<u8>,
    norm: BindingNorm,
}

impl Sink {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            norm: BindingNorm::default(),
        }
    }

    fn tag(&mut self, t: u8) {
        self.buf.push(t);
    }

    fn u32(&mut self, n: u32) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    fn f64(&mut self, n: f64) {
        self.buf.extend_from_slice(&n.to_bits().to_le_bytes());
    }

    #[allow(clippy::cast_possible_truncation)]
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn ty(&mut self, ty: &Type) {
        self.str(&render_type(ty));
    }

    fn name(&mut self, qn: &QualifiedName) {
        self.str(&render_name(qn));
    }

    fn local(&mut self, id: BindingId) {
        let n = self.norm.get(id);
        self.u32(n);
    }

    fn finish(self) -> [u8; 32] {
        *blake3::hash(&self.buf).as_bytes()
    }
}

/// Hash a callable body (its parameters, so `Local` refs into them
/// normalize, then the body expression). Used for impl-method and
/// ability-default bodies.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn hash_body(params: &[Param], body: &Expr) -> [u8; 32] {
    let mut sink = Sink::new();
    sink.buf.extend_from_slice(BODY_DOMAIN);
    sink.u32(params.len() as u32);
    for p in params {
        // Establish each param's normalized id; skip its name (renaming a
        // parameter is not a content change).
        sink.local(p.id);
        write_opt_type(&mut sink, p.ty.as_ref());
    }
    write_expr(&mut sink, body);
    sink.finish()
}

/// The resolved-AST hash of a whole module: every item, private ones
/// included. Order-independent (per-item hashes are sorted before folding)
/// and span-free, so it flips on any item change but not on reformatting or
/// declaration reordering.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn module_ast_hash(module: &crate::ast::Module) -> blake3::Hash {
    let mut item_hashes: Vec<[u8; 32]> = module.items.iter().map(hash_item).collect();
    item_hashes.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    hasher.update(MODULE_DOMAIN);
    hasher.update(&(item_hashes.len() as u32).to_le_bytes());
    for h in &item_hashes {
        hasher.update(h);
    }
    hasher.finalize()
}

/// Structural hash of a single item (fresh binding scope).
fn hash_item(item: &Item) -> [u8; 32] {
    let mut sink = Sink::new();
    sink.buf.extend_from_slice(ITEM_DOMAIN);
    write_item(&mut sink, item);
    sink.finish()
}

#[allow(clippy::too_many_lines)]
fn write_item(sink: &mut Sink, item: &Item) {
    match &item.kind {
        ItemKind::Function(f) => {
            sink.tag(0);
            sink.str(&f.name);
            sink.buf.push(u8::from(f.is_public));
            write_type_params(sink, &f.type_params);
            sink.u32(f.params.len() as u32);
            for p in &f.params {
                sink.local(p.id);
                write_opt_type(sink, p.ty.as_ref());
            }
            write_opt_type(sink, f.ret_ty.as_ref());
            write_qn_list(sink, &f.abilities);
            write_expr(sink, &f.body);
        }
        ItemKind::ExternFn(e) => {
            sink.tag(1);
            sink.str(&e.name);
            sink.buf.push(u8::from(e.is_public));
            write_type_params(sink, &e.type_params);
            sink.u32(e.params.len() as u32);
            for p in &e.params {
                write_opt_type(sink, p.ty.as_ref());
            }
            sink.ty(&e.ret_ty);
        }
        ItemKind::Const(c) => {
            sink.tag(2);
            sink.str(&c.name);
            sink.buf.push(u8::from(c.is_public));
            write_opt_type(sink, c.ty.as_ref());
            write_expr(sink, &c.value);
        }
        ItemKind::Struct(s) => {
            sink.tag(3);
            sink.str(&s.name);
            sink.buf.push(u8::from(s.is_public));
            sink.buf.push(u8::from(s.is_extern));
            match &s.unique_id {
                Some(u) => {
                    sink.buf.push(1);
                    sink.str(&u.to_string());
                }
                None => sink.buf.push(0),
            }
            write_type_params(sink, &s.type_params);
            sink.ty(&s.ty);
        }
        ItemKind::TypeAlias(t) => {
            sink.tag(4);
            sink.str(&t.name);
            sink.buf.push(u8::from(t.is_public));
            write_type_params(sink, &t.type_params);
            sink.ty(&t.ty);
        }
        ItemKind::Enum(e) => {
            sink.tag(5);
            sink.str(&e.name);
            sink.buf.push(u8::from(e.is_public));
            sink.str(&e.uuid.to_string());
            write_type_params(sink, &e.type_params);
            sink.u32(e.variants.len() as u32);
            for v in &e.variants {
                sink.str(&v.name);
                write_opt_type(sink, v.payload.as_ref());
            }
        }
        ItemKind::Ability(a) => {
            sink.tag(6);
            sink.str(&a.name);
            sink.buf.push(u8::from(a.is_public));
            sink.str(&a.uuid.to_string());
            write_qn_list(sink, &a.dependencies);
            sink.u32(a.methods.len() as u32);
            for m in &a.methods {
                sink.str(&m.name);
                write_type_params(sink, &m.type_params);
                sink.u32(m.params.len() as u32);
                for p in &m.params {
                    write_opt_type(sink, p.ty.as_ref());
                }
                sink.ty(&m.ret_ty);
                match &m.body {
                    Some(body) => {
                        sink.buf.push(1);
                        let h = hash_body(&m.params, body);
                        sink.buf.extend_from_slice(&h);
                    }
                    None => sink.buf.push(0),
                }
            }
        }
        ItemKind::Trait(t) => {
            sink.tag(7);
            sink.str(&t.name);
            sink.buf.push(u8::from(t.is_public));
            sink.str(&t.uuid.to_string());
            write_type_params(sink, &t.type_params);
            write_qn_list(sink, &t.supertraits);
            sink.u32(t.assoc_types.len() as u32);
            for a in &t.assoc_types {
                sink.str(&a.name);
            }
            sink.u32(t.methods.len() as u32);
            for m in &t.methods {
                sink.str(&m.name);
                sink.buf.push(u8::from(m.has_self));
                write_type_params(sink, &m.type_params);
                sink.u32(m.params.len() as u32);
                for (pname, pty) in &m.params {
                    sink.str(pname);
                    sink.ty(pty);
                }
                sink.ty(&m.ret_ty);
                write_qn_list(sink, &m.abilities);
            }
        }
        ItemKind::Impl(i) => {
            sink.tag(8);
            match &i.trait_name {
                Some(tn) => {
                    sink.buf.push(1);
                    sink.name(&tn.name);
                    #[allow(clippy::cast_possible_truncation)]
                    sink.u32(tn.args.len() as u32);
                    for arg in &tn.args {
                        sink.ty(arg);
                    }
                }
                None => sink.buf.push(0),
            }
            sink.ty(&i.for_type);
            write_type_params(sink, &i.type_params);
            sink.u32(i.assoc_types.len() as u32);
            for a in &i.assoc_types {
                sink.str(&a.name);
                sink.ty(&a.ty);
            }
            sink.u32(i.methods.len() as u32);
            for m in &i.methods {
                sink.str(&m.name);
                sink.buf.push(u8::from(m.has_self));
                sink.u32(m.params.len() as u32);
                for p in &m.params {
                    write_opt_type(sink, p.ty.as_ref());
                }
                write_opt_type(sink, m.ret_ty.as_ref());
                write_qn_list(sink, &m.abilities);
                let h = hash_body(&m.params, &m.body);
                sink.buf.extend_from_slice(&h);
            }
        }
        ItemKind::Use(u) => {
            sink.tag(9);
            write_use(sink, u);
        }
    }
}

fn write_type_params(sink: &mut Sink, tps: &[crate::ast::TypeParam]) {
    #[allow(clippy::cast_possible_truncation)]
    sink.u32(tps.len() as u32);
    for tp in tps {
        sink.str(&tp.name);
        sink.buf.push(u8::from(tp.is_ability));
        write_trait_ref_list(sink, &tp.bounds);
    }
}

fn write_trait_ref_list(sink: &mut Sink, bounds: &[crate::ast::TraitRef]) {
    #[allow(clippy::cast_possible_truncation)]
    sink.u32(bounds.len() as u32);
    for bound in bounds {
        sink.name(&bound.name);
        #[allow(clippy::cast_possible_truncation)]
        sink.u32(bound.args.len() as u32);
        for arg in &bound.args {
            sink.ty(arg);
        }
    }
}

fn write_qn_list(sink: &mut Sink, names: &[QualifiedName]) {
    #[allow(clippy::cast_possible_truncation)]
    sink.u32(names.len() as u32);
    for qn in names {
        sink.name(qn);
    }
}

fn write_opt_type(sink: &mut Sink, ty: Option<&Type>) {
    match ty {
        Some(t) => {
            sink.buf.push(1);
            sink.ty(t);
        }
        None => sink.buf.push(0),
    }
}

fn write_use(sink: &mut Sink, u: &UseDef) {
    sink.buf.push(u8::from(u.is_public));
    sink.buf.push(match u.prefix {
        crate::ast::UsePrefix::Pkg => 0,
        crate::ast::UsePrefix::Core => 1,
        crate::ast::UsePrefix::Self_ => 2,
        crate::ast::UsePrefix::Super(_) => 3,
        crate::ast::UsePrefix::Local => 4,
    });
    if let crate::ast::UsePrefix::Super(n) = u.prefix {
        #[allow(clippy::cast_possible_truncation)]
        sink.u32(n as u32);
    }
    #[allow(clippy::cast_possible_truncation)]
    sink.u32(u.path.len() as u32);
    for (seg, _) in &u.path {
        sink.str(seg);
    }
    match &u.alias {
        Some((a, _)) => {
            sink.buf.push(1);
            sink.str(a);
        }
        None => sink.buf.push(0),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Expression / statement / pattern walk
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
fn write_expr(sink: &mut Sink, expr: &Expr) {
    // Deliberately ignores `expr.ty` and `expr.dicts` (checker-populated,
    // absent at interface time) and all spans.
    match &expr.kind {
        ExprKind::Unit => sink.tag(0),
        ExprKind::Bool(b) => {
            sink.tag(1);
            sink.buf.push(u8::from(*b));
        }
        ExprKind::Number(n) => {
            sink.tag(2);
            sink.f64(*n);
        }
        ExprKind::String(s) => {
            sink.tag(3);
            sink.str(s);
        }
        ExprKind::Local(id) => {
            sink.tag(4);
            sink.local(*id);
        }
        ExprKind::Name(qn) => {
            sink.tag(5);
            sink.name(qn);
        }
        ExprKind::Tuple(exprs) => {
            sink.tag(6);
            write_exprs(sink, exprs);
        }
        ExprKind::TupleIndex(e, idx) => {
            sink.tag(7);
            write_expr(sink, e);
            sink.u32(*idx);
        }
        ExprKind::Record(fields) => {
            sink.tag(8);
            sink.u32(fields.len() as u32);
            for (name, e) in fields {
                sink.str(name);
                write_expr(sink, e);
            }
        }
        ExprKind::TypedRecord { type_name, fields } => {
            sink.tag(9);
            sink.name(type_name);
            sink.u32(fields.len() as u32);
            for (name, e) in fields {
                sink.str(name);
                write_expr(sink, e);
            }
        }
        ExprKind::RecordField(e, name) => {
            sink.tag(10);
            write_expr(sink, e);
            sink.str(name);
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            sink.tag(11);
            write_expr(sink, receiver);
            sink.str(method);
            write_exprs(sink, args);
        }
        ExprKind::List(exprs) => {
            sink.tag(12);
            write_exprs(sink, exprs);
        }
        ExprKind::Binary {
            op, left, right, ..
        } => {
            sink.tag(13);
            sink.buf.push(binop_tag(*op));
            write_expr(sink, left);
            write_expr(sink, right);
        }
        ExprKind::Unary(op, e) => {
            sink.tag(14);
            sink.buf.push(match op {
                UnaryOp::Neg => 0,
                UnaryOp::Not => 1,
            });
            write_expr(sink, e);
        }
        ExprKind::If(c, t, e) => {
            sink.tag(15);
            write_expr(sink, c);
            write_expr(sink, t);
            write_opt_expr(sink, e.as_deref());
        }
        ExprKind::Match(scrut, arms) => {
            sink.tag(16);
            write_expr(sink, scrut);
            sink.u32(arms.len() as u32);
            for arm in arms {
                write_arm(sink, arm);
            }
        }
        ExprKind::Block(stmts, result) => {
            sink.tag(17);
            sink.u32(stmts.len() as u32);
            for stmt in stmts {
                write_stmt(sink, stmt);
            }
            write_opt_expr(sink, result.as_deref());
        }
        ExprKind::Lambda(Lambda { params, body }) => {
            sink.tag(18);
            sink.u32(params.len() as u32);
            for p in params {
                sink.local(p.id);
                write_opt_type(sink, p.ty.as_ref());
            }
            write_expr(sink, body);
        }
        ExprKind::Call(callee, args) => {
            sink.tag(19);
            write_expr(sink, callee);
            write_exprs(sink, args);
        }
        ExprKind::Perform(AbilityCall {
            ability,
            method,
            args,
            ..
        }) => {
            sink.tag(20);
            // A bare-method perform (`seed!(…)`) hashes distinctly from a
            // spelled `seed::seed!(…)` until resolution; once the resolve
            // pass fills the ability's canonical reference it hashes
            // exactly like the qualified spelling.
            match ability {
                Some(ability) => {
                    sink.tag(1);
                    sink.name(ability);
                }
                None => sink.tag(0),
            }
            sink.str(method);
            write_exprs(sink, args);
        }
        ExprKind::Handle(HandleExpr {
            handlers,
            body,
            else_clause,
        }) => {
            sink.tag(21);
            write_exprs(sink, handlers);
            write_expr(sink, body);
            write_opt_expr(sink, else_clause.as_deref());
        }
        ExprKind::Resume(e) => {
            sink.tag(22);
            write_expr(sink, e);
        }
        ExprKind::HandlerLiteral(HandlerLiteralExpr { methods, .. }) => {
            sink.tag(23);
            sink.u32(methods.len() as u32);
            for m in methods {
                sink.name(&m.ability);
                sink.str(&m.method);
                sink.u32(m.params.len() as u32);
                for p in &m.params {
                    sink.local(p.id);
                    write_opt_type(sink, p.ty.as_ref());
                }
                write_expr(sink, &m.body);
            }
        }
        ExprKind::Sandbox(SandboxExpr {
            allowed_abilities,
            body,
            ..
        }) => {
            sink.tag(24);
            write_qn_list(sink, allowed_abilities);
            write_expr(sink, body);
        }
    }
}

fn write_exprs(sink: &mut Sink, exprs: &[Expr]) {
    #[allow(clippy::cast_possible_truncation)]
    sink.u32(exprs.len() as u32);
    for e in exprs {
        write_expr(sink, e);
    }
}

fn write_opt_expr(sink: &mut Sink, expr: Option<&Expr>) {
    match expr {
        Some(e) => {
            sink.buf.push(1);
            write_expr(sink, e);
        }
        None => sink.buf.push(0),
    }
}

fn write_stmt(sink: &mut Sink, stmt: &Stmt) {
    match &stmt.kind {
        StmtKind::Let(binding) => {
            sink.tag(0);
            sink.local(binding.id);
            write_opt_type(sink, binding.ty.as_ref());
            write_expr(sink, &binding.init);
        }
        StmtKind::Expr(e) => {
            sink.tag(1);
            write_expr(sink, e);
        }
        StmtKind::Use(u) => {
            sink.tag(2);
            write_use(sink, u);
        }
        StmtKind::Const(c) => {
            sink.tag(3);
            sink.local(c.id);
            write_opt_type(sink, c.ty.as_ref());
            write_expr(sink, &c.value);
        }
    }
}

fn write_arm(sink: &mut Sink, arm: &MatchArm) {
    write_pattern(sink, &arm.pattern);
    write_opt_expr(sink, arm.guard.as_ref());
    write_expr(sink, &arm.body);
}

fn write_pattern(sink: &mut Sink, pat: &Pattern) {
    match &pat.kind {
        PatternKind::Wildcard => sink.tag(0),
        PatternKind::Binding(id, _name) => {
            // Skip the name (renaming a binding is not a content change);
            // the normalized id ties references to it.
            sink.tag(1);
            sink.local(*id);
        }
        PatternKind::Literal(lit) => {
            sink.tag(2);
            write_literal(sink, lit);
        }
        PatternKind::Tuple(pats) => {
            sink.tag(3);
            #[allow(clippy::cast_possible_truncation)]
            sink.u32(pats.len() as u32);
            for p in pats {
                write_pattern(sink, p);
            }
        }
        PatternKind::Record(fields) => {
            sink.tag(4);
            #[allow(clippy::cast_possible_truncation)]
            sink.u32(fields.len() as u32);
            for (name, p) in fields {
                sink.str(name);
                write_pattern(sink, p);
            }
        }
        PatternKind::Variant(qn, inner) => {
            sink.tag(5);
            sink.name(qn);
            match inner {
                Some(p) => {
                    sink.buf.push(1);
                    write_pattern(sink, p);
                }
                None => sink.buf.push(0),
            }
        }
    }
}

fn write_literal(sink: &mut Sink, lit: &Literal) {
    match lit {
        Literal::Unit => sink.tag(0),
        Literal::Bool(b) => {
            sink.tag(1);
            sink.buf.push(u8::from(*b));
        }
        Literal::Number(n) => {
            sink.tag(2);
            sink.f64(*n);
        }
        Literal::String(s) => {
            sink.tag(3);
            sink.str(s);
        }
    }
}

const fn binop_tag(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Add => 0,
        BinaryOp::Sub => 1,
        BinaryOp::Mul => 2,
        BinaryOp::Div => 3,
        BinaryOp::Mod => 4,
        BinaryOp::Eq => 5,
        BinaryOp::Ne => 6,
        BinaryOp::Lt => 7,
        BinaryOp::Le => 8,
        BinaryOp::Gt => 9,
        BinaryOp::Ge => 10,
        BinaryOp::And => 11,
        BinaryOp::Or => 12,
    }
}
