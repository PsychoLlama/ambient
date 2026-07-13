//! Compile-order edges from type-directed dispatch.
//!
//! An inherent-method call (`x.floor()`), an overloaded operator on a
//! nominal type (`a + b` where `a: Money`), or an associated-function call
//! (`List::range(1, 4)`) links against the callee's compiled body under a
//! content-addressed dispatch symbol (`<uuid>::method`), exactly like a
//! direct cross-module function call. The caller therefore depends on the
//! defining module compiling first, so its final hash is in the linking
//! table.
//!
//! But unlike a direct call, the reference is resolved by the *checker* from
//! the receiver's inferred type, not by the resolve pass — so it never became
//! a `use`/value dependency edge. The caller can then be ordered before the
//! definer, the symbol is missing from the linking table, and the call fails
//! to link (`undefined function: <uuid>::method`).
//!
//! For user modules this never bites: a foreign type is always named by a
//! qualified path or a `use` (both are dependency edges, see
//! [`crate::resolve`]), and the whole core library is compiled before any
//! user module. It bites *within* a single compile group — the core library,
//! the platform interface — where prelude/built-in types (`Number`, `List`)
//! are referenced bare, which by design adds no dependency edge (a bare type
//! reference must not manufacture compile-order cycles; see
//! `resolve::types`). This module recovers exactly the edges a type-directed
//! dispatch needs, from the checked ASTs, so the definer compiles first.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Expr, ExprKind, Item, ItemKind, Module, ResolvedMethod, walk_exprs};

/// Given each group module's checked AST keyed by its ordering key (the
/// string the compile-order graph uses), return the extra dependency edges
/// type-directed dispatch demands: `caller_key -> [definer_key, ...]`.
///
/// An edge is added whenever a module references a dispatch symbol that a
/// *different* group module defines. Symbols defined outside the group
/// (already-compiled dependencies like a prior core module) need no edge —
/// their hashes are already in the linking table.
#[must_use]
pub fn dispatch_edges(checked: &[(String, &Module)]) -> HashMap<String, Vec<String>> {
    // symbol -> the group module that defines it. Impl-method dispatch
    // symbols are globally unique, so one owner per symbol.
    let mut owner: HashMap<Arc<str>, String> = HashMap::new();
    for (key, module) in checked {
        for symbol in defined_symbols(module) {
            owner.insert(Arc::clone(symbol), key.clone());
        }
    }

    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for (key, module) in checked {
        let mut referenced = Vec::new();
        collect_referenced_symbols(module, &mut referenced);
        for symbol in referenced {
            if let Some(definer) = owner.get(&symbol)
                && definer != key
            {
                let deps = edges.entry(key.clone()).or_default();
                if !deps.contains(definer) {
                    deps.push(definer.clone());
                }
            }
        }
    }
    edges
}

/// Every impl-method dispatch symbol a module *defines* (inherent and trait
/// impls alike; both compile under `ImplMethod::resolved_symbol`).
fn defined_symbols(module: &Module) -> impl Iterator<Item = &Arc<str>> {
    module.items.iter().flat_map(|item| {
        let methods = match &item.kind {
            ItemKind::Impl(def) => def.methods.as_slice(),
            _ => &[],
        };
        methods.iter().filter_map(|m| m.resolved_symbol.as_ref())
    })
}

/// Collect every dispatch symbol a module *references* in its bodies:
///
/// - method calls (`x.floor()`) and overloaded operators (`a + b`), whose
///   [`ResolvedMethod::Symbol`] the checker recorded;
/// - rewritten associated calls (`List::range(...)`), which the checker
///   lowers to a bare [`ExprKind::Name`] whose spelling *is* the dispatch
///   symbol (an ordinary name never contains `::`).
fn collect_referenced_symbols(module: &Module, out: &mut Vec<Arc<str>>) {
    let mut visit = |e: &Expr| match &e.kind {
        ExprKind::MethodCall {
            resolved_method: Some(ResolvedMethod::Symbol(s)),
            ..
        }
        | ExprKind::Binary {
            resolved_op: Some(ResolvedMethod::Symbol(s)),
            ..
        } => out.push(Arc::clone(s)),
        ExprKind::Name(name) if name.path.is_empty() && name.name.contains("::") => {
            out.push(Arc::clone(&name.name));
        }
        _ => {}
    };
    for item in &module.items {
        for body in item_bodies(item) {
            walk_exprs(body, &mut visit);
        }
    }
}

/// The expression roots inside an item: a function body, each impl-method
/// body, and each ability-method default implementation.
fn item_bodies(item: &Item) -> Vec<&Expr> {
    match &item.kind {
        ItemKind::Function(def) => vec![&def.body],
        ItemKind::Impl(def) => def.methods.iter().map(|m| &m.body).collect(),
        ItemKind::Ability(def) => def.methods.iter().filter_map(|m| m.body.as_ref()).collect(),
        _ => Vec::new(),
    }
}
