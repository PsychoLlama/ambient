//! Module-level reachability for lazy (`ambient run`) builds.
//!
//! A whole-package build compiles every module under `src/`. But `ambient run`
//! only needs the modules the entry point's behavior can actually reach, so it
//! asks [`reachable_module_ids`] for that closure and compiles nothing else
//! (see the build loop in [`super::build_package`]). `ambient check`, the LSP,
//! `ambient compile`, and `ambient dev` stay whole-package — only `run` prunes.
//!
//! # What "reachable" must cover
//!
//! Runtime behavior flows across modules through more than `use` edges, and the
//! closure has to be a **sound over-approximation** of every such channel or a
//! lazy run would link-fail (`undefined function: <uuid>::method`) or silently
//! drop a dispatch. The channels and how each is covered:
//!
//! - **Resolve-pass dependencies** (`use`, inline qualified paths, enum-variant
//!   construction, foreign consts, ability performs, and *ability default-impl
//!   bodies* — which the resolve pass walks like any function body): a plain
//!   forward closure over the resolve-dep graph (`dep_ids`). This already
//!   covers abilities end to end — you cannot perform an ability without naming
//!   it (a dep on its module), and that module's default-impl body carries its
//!   own deps.
//!
//! - **Type-directed dispatch / trait coherence** (`x.method()`, `a + b` on a
//!   nominal type, `Type::assoc(..)`): the checker resolves these to a
//!   content-addressed `<type-uuid>::<trait-uuid>::method` symbol defined in
//!   whichever module wrote the `impl` block — which the dispatcher **need not
//!   import** (there is no orphan rule; an `impl Show for Widget` can live in a
//!   third module). The resolve-dep graph misses this edge. We recover it
//!   without type-checking (checking an unreachable module would violate the
//!   diagnostics policy) via the key structural fact: to dispatch an impl for a
//!   type `T`, reachable code must hold a `T` value, so `T`'s defining module
//!   is always reachable. So we make each impl-defining module reachable *from
//!   its impl's target type's module* (a reverse edge). When the target type is
//!   a builtin/prelude type (or a blanket/param impl), reachable code can hold
//!   the value without any package dep, so we cannot prove the impl unreachable
//!   and include the module **unconditionally**.
//!
//! This yields a superset of the true reachable set: spurious inclusion only
//! costs compile time, never correctness. It is *not* item/FQN-grain — a
//! reachable module compiles whole (the checker's intra-module coupling).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use uuid::Uuid;

use crate::ast::{ItemKind, Module};
use crate::types::Type;

/// The compile-order graph the build loop feeds to
/// [`compilation_order`](super::pipeline::compilation_order): the resolve-
/// dependency graph `deps` augmented with **structural** type-directed dispatch
/// edges, so an orphan trait impl compiles before any module that may dispatch
/// it.
///
/// ## Why the resolve graph is not enough
///
/// A cross-module method / operator / associated call links against a
/// content-addressed `<uuid>::method` dispatch symbol defined in whichever
/// module wrote the `impl` — which the dispatcher **need not import** (there is
/// no orphan rule). That reference is resolved by the *checker*, not the resolve
/// pass, so it never became a `deps` edge. If the impl module happens to sort
/// after the dispatcher, the symbol is missing at link time (`undefined
/// function: <uuid>::method`). The core/platform groups avoid this with
/// [`crate::dispatch_deps`] (edges recovered from *checked* ASTs), but a user
/// build cannot check every module up front — unreached modules must not be
/// checked (diagnostics policy) and cache-hit modules should not be re-checked.
///
/// ## The structural edge
///
/// We recover a **conservative superset** of the real edges from the resolved
/// (unchecked) ASTs, via the same structural fact [`reachable_module_ids`]
/// uses: to dispatch an impl for type `T`, reachable code must hold a `T`, so
/// any dispatcher of that impl transitively resolve-depends on `T`'s defining
/// module. So for each impl module `I` with a *package* target type declared in
/// module `Tmod`, every module that transitively depends on `Tmod` gets an edge
/// to `I` (`dispatcher -> I`, so `I` compiles first). For an impl on a
/// builtin/prelude type or a blanket impl — which reachable code can hold with
/// no package dep — every module is a candidate. A superset only over-orders
/// (spurious edges cost nothing as long as the graph stays acyclic).
///
/// ## Cycle policy
///
/// Structural edges only *order*; they never enter cycle **diagnostics** (those
/// stay on `deps` alone — see [`crate::module_cycles`]). A candidate that `I`
/// itself transitively resolve-depends on is dropped up front: that ordering is
/// unsatisfiable in a single pass (`I` needs its dep compiled first *and* the
/// dep needs `I`'s symbol) and would only manufacture a cycle a real dispatch
/// edge would not. As a final guard, if the augmented graph is cyclic while
/// `deps` alone is not, the structural edges are discarded wholesale and the
/// plain resolve order is used — such a program has a genuinely cyclic dispatch
/// dependency single-pass linking cannot satisfy, and it fails to link exactly
/// as it did before this pass (never a new false cycle *error*).
///
/// `modules` carries each package module's resolved AST keyed by the same string
/// `deps` uses (its dotted [`ModulePath`](crate::module_path::ModulePath)).
pub(super) fn dispatch_ordering_graph(
    deps: &BTreeMap<String, Vec<String>>,
    modules: &[(String, &Module)],
) -> BTreeMap<String, Vec<String>> {
    let extra = structural_dispatch_edges(deps, modules);
    if extra.is_empty() {
        return deps.clone();
    }
    let mut augmented = deps.clone();
    for (caller, definers) in &extra {
        let slot = augmented.entry(caller.clone()).or_default();
        for definer in definers {
            if !slot.contains(definer) {
                slot.push(definer.clone());
            }
        }
    }
    // `deps` alone is acyclic (the build rejects import cycles before ordering),
    // so any cycle here was introduced by a structural edge; fall back rather
    // than order a genuinely-cyclic dispatch dependency arbitrarily.
    if crate::module_cycles::detect_import_cycles(&augmented).is_empty() {
        augmented
    } else {
        deps.clone()
    }
}

/// The structural dispatch ordering edges (`dispatcher -> impl-module`) derived
/// from resolved ASTs. See [`dispatch_ordering_graph`] for the reasoning and
/// the cycle policy the caller applies on top.
fn structural_dispatch_edges(
    deps: &BTreeMap<String, Vec<String>>,
    modules: &[(String, &Module)],
) -> BTreeMap<String, Vec<String>> {
    // Types each module declares, for resolving an impl's target-type head to
    // the module(s) that could construct it.
    let declared: HashMap<&str, Vec<TypeDecl>> = modules
        .iter()
        .map(|(id, ast)| (id.as_str(), declared_types(ast)))
        .collect();
    let all_keys: BTreeSet<&str> = modules.iter().map(|(id, _)| id.as_str()).collect();
    // Reverse resolve edges (`dep -> dependents`), for walking from a target
    // type's module out to every module that transitively depends on it.
    let mut rev: HashMap<&str, Vec<&str>> = HashMap::new();
    for (node, node_deps) in deps {
        for dep in node_deps {
            if deps.contains_key(dep) {
                rev.entry(dep.as_str()).or_default().push(node.as_str());
            }
        }
    }

    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (impl_id, ast) in modules {
        // Anchor the target-type search to this module plus its direct resolve
        // deps — where any type it names must be defined.
        let mut scope: Vec<&str> = deps
            .get(impl_id)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        scope.push(impl_id.as_str());
        // Modules this impl module transitively resolve-depends on: forcing the
        // impl before one of them is unsatisfiable, so such candidates are cut.
        let impl_deps = forward_closure(deps, impl_id);

        for item in &ast.items {
            let ItemKind::Impl(imp) = &item.kind else {
                continue;
            };
            let candidates = dispatchers_of(&imp.for_type, &scope, &declared, &rev, &all_keys);
            for cand in candidates {
                if cand == *impl_id || impl_deps.contains(&cand) {
                    continue;
                }
                let slot = edges.entry(cand).or_default();
                if !slot.contains(impl_id) {
                    slot.push(impl_id.clone());
                }
            }
        }
    }
    edges
}

/// The candidate dispatcher modules for one impl block's target type: every
/// module that can hold a value of that type. For a *package* target type, that
/// is the modules transitively depending on the type's module (walking `rev`);
/// for a builtin/prelude type or a blanket/param impl, any module qualifies.
fn dispatchers_of(
    for_type: &Type,
    scope: &[&str],
    declared: &HashMap<&str, Vec<TypeDecl>>,
    rev: &HashMap<&str, Vec<&str>>,
    all_keys: &BTreeSet<&str>,
) -> BTreeSet<String> {
    let Some((head_name, head_uuid)) = type_head(for_type) else {
        // A blanket/param impl (`impl<T> Show for T`) dispatches on any type.
        return all_keys.iter().map(|k| (*k).to_string()).collect();
    };
    let anchors: Vec<&str> = scope
        .iter()
        .copied()
        .filter(|d| {
            declared.get(d).is_some_and(|types| {
                types
                    .iter()
                    .any(|t| type_matches(t, head_name.as_ref(), head_uuid))
            })
        })
        .collect();
    if anchors.is_empty() {
        // The target type is not a package type — a builtin/prelude type that
        // reachable code can hold with no dependency edge, so any module could
        // dispatch this impl.
        return all_keys.iter().map(|k| (*k).to_string()).collect();
    }
    let mut out = BTreeSet::new();
    for anchor in anchors {
        reverse_reachable(rev, anchor, &mut out);
    }
    out
}

/// Every package module `start` transitively resolve-depends on (excluding
/// `start`). Non-node targets (core/platform) are ignored.
fn forward_closure(deps: &BTreeMap<String, Vec<String>>, start: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<&str> = deps
        .get(start)
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    while let Some(node) = stack.pop() {
        if !deps.contains_key(node) || !seen.insert(node.to_string()) {
            continue;
        }
        stack.extend(deps.get(node).into_iter().flatten().map(String::as_str));
    }
    seen
}

/// Every module that transitively depends on `target` (its ancestors in the
/// resolve graph), accumulated into `out`. `target` itself is excluded.
fn reverse_reachable(rev: &HashMap<&str, Vec<&str>>, target: &str, out: &mut BTreeSet<String>) {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![target];
    while let Some(node) = stack.pop() {
        for &parent in rev.get(node).into_iter().flatten() {
            if seen.insert(parent) {
                out.insert(parent.to_string());
                stack.push(parent);
            }
        }
    }
}

/// A resolved package module the reachability pass reads: its canonical module
/// identity (matching the `dep_ids` keys) and its resolved AST.
pub(super) struct PackageModule<'a> {
    /// Canonical module identity (`workspace::pkg::utils`), the `dep_ids` key.
    pub id: String,
    /// The module's resolved AST.
    pub ast: &'a Module,
}

/// A nominal type a module declares: its name and (for enums / `unique` structs)
/// its uuid, for matching an impl's target-type head.
struct TypeDecl {
    name: Arc<str>,
    uuid: Option<Uuid>,
}

/// The set of package module ids reachable from `entry`, or `None` when no
/// package module declares a top-level function matching `entry` (the caller
/// then builds the whole package — a safe fallback, never a lazy prune).
///
/// `dep_ids` is the resolve-pass dependency graph keyed by canonical module
/// identity (a superset of `modules`; values may name core/platform modules,
/// which are ignored — they always build). The returned set is a subset of
/// `dep_ids`' package keys.
pub(super) fn reachable_module_ids(
    entry: &str,
    dep_ids: &BTreeMap<String, Vec<String>>,
    modules: &[PackageModule<'_>],
) -> Option<BTreeSet<String>> {
    let seeds = entry_seeds(entry, modules);
    if seeds.is_empty() {
        return None;
    }

    // Every nominal type each package module declares, for resolving an impl's
    // target-type head to the module(s) that could construct it.
    let declared: HashMap<&str, Vec<TypeDecl>> = modules
        .iter()
        .map(|m| (m.id.as_str(), declared_types(m.ast)))
        .collect();

    // The impl channel, split two ways: modules that must always be reachable
    // (an impl on a builtin/prelude type or a blanket/param impl — undispatchable
    // to prove unreachable), and reverse edges `type-module -> impl-module` so an
    // impl comes in exactly when a type it dispatches on does.
    let mut unconditional: BTreeSet<String> = BTreeSet::new();
    let mut pulled_by: HashMap<String, BTreeSet<String>> = HashMap::new();
    for m in modules {
        collect_impl_edges(m, dep_ids, &declared, &mut unconditional, &mut pulled_by);
    }

    // One worklist over both edge kinds: forward resolve-deps, and the impl
    // reverse edges fired when a target-type module enters the set.
    let mut reachable: BTreeSet<String> = seeds;
    reachable.append(&mut unconditional.clone());
    let mut frontier: Vec<String> = reachable.iter().cloned().collect();
    while let Some(m) = frontier.pop() {
        for dep in dep_ids.get(&m).into_iter().flatten() {
            // Only follow edges to package modules; core/platform always build.
            if dep_ids.contains_key(dep) && reachable.insert(dep.clone()) {
                frontier.push(dep.clone());
            }
        }
        if let Some(impls) = pulled_by.get(&m) {
            for u in impls {
                if reachable.insert(u.clone()) {
                    frontier.push(u.clone());
                }
            }
        }
    }
    Some(reachable)
}

/// Package modules declaring a top-level function that matches `entry`,
/// mirroring the CLI's entry resolution: an exact fully-qualified match, a
/// `::{entry}` suffix, or (for a bare `entry` like the default `run`) the plain
/// function name.
fn entry_seeds(entry: &str, modules: &[PackageModule<'_>]) -> BTreeSet<String> {
    let suffix = format!("::{entry}");
    let mut seeds = BTreeSet::new();
    for m in modules {
        for item in &m.ast.items {
            if let ItemKind::Function(f) = &item.kind {
                let fqn = format!("{}::{}", m.id, f.name);
                if fqn == entry
                    || fqn.ends_with(&suffix)
                    || (!entry.contains("::") && &*f.name == entry)
                {
                    seeds.insert(m.id.clone());
                }
            }
        }
    }
    seeds
}

/// Every nominal type (`struct`/`enum`) a module declares, with its uuid.
fn declared_types(module: &Module) -> Vec<TypeDecl> {
    let mut out = Vec::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Struct(s) => out.push(TypeDecl {
                name: Arc::clone(&s.name),
                uuid: s.unique_id,
            }),
            ItemKind::Enum(e) => out.push(TypeDecl {
                name: Arc::clone(&e.name),
                uuid: Some(e.uuid),
            }),
            _ => {}
        }
    }
    out
}

/// Fold one module's impl blocks into the reachability edges: an impl whose
/// target type is a package type contributes a reverse edge from that type's
/// module; an impl on a builtin/prelude type (or a blanket/param impl) makes the
/// module unconditionally reachable.
fn collect_impl_edges(
    m: &PackageModule<'_>,
    dep_ids: &BTreeMap<String, Vec<String>>,
    declared: &HashMap<&str, Vec<TypeDecl>>,
    unconditional: &mut BTreeSet<String>,
    pulled_by: &mut HashMap<String, BTreeSet<String>>,
) {
    // Candidate anchor modules for a type reference: this module plus its
    // resolve deps (which is where any type it names must be defined). Bounding
    // the search here keeps a same-named type in an unrelated module from
    // manufacturing a spurious anchor.
    let mut scope: Vec<&str> = dep_ids
        .get(&m.id)
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    scope.push(m.id.as_str());

    for item in &m.ast.items {
        let ItemKind::Impl(imp) = &item.kind else {
            continue;
        };
        let Some((head_name, head_uuid)) = type_head(&imp.for_type) else {
            // A blanket/param impl (`impl<T> Show for T`) dispatches on any type;
            // it must always be present.
            unconditional.insert(m.id.clone());
            continue;
        };
        let anchors: Vec<&str> = scope
            .iter()
            .copied()
            .filter(|d| {
                declared.get(d).is_some_and(|types| {
                    types
                        .iter()
                        .any(|t| type_matches(t, head_name.as_ref(), head_uuid))
                })
            })
            .collect();
        if anchors.is_empty() {
            // The target type is not a package type — a builtin/prelude type
            // reachable code can always hold. We cannot prove the impl
            // unreachable, so include the module unconditionally.
            unconditional.insert(m.id.clone());
        } else {
            for anchor in anchors {
                pulled_by
                    .entry(anchor.to_string())
                    .or_default()
                    .insert(m.id.clone());
            }
        }
    }
}

/// The nominal head of an impl's target type — its name and (enum / `unique`
/// struct) uuid — or `None` for a non-nominal head (a type parameter, tuple,
/// function type, …), which signals a blanket impl.
fn type_head(ty: &Type) -> Option<(Option<Arc<str>>, Option<Uuid>)> {
    match ty {
        Type::Named(n) => Some((Some(Arc::clone(&n.name)), n.uuid)),
        Type::Nominal(nom) => Some((nom.name.clone(), Some(nom.uuid))),
        _ => None,
    }
}

/// Whether a declared type matches an impl's target-type head: uuid-equal when
/// both carry one (the strict nominal test), else name-equal (a struct
/// annotation carries no uuid before checking). Over-matching only over-includes.
fn type_matches(decl: &TypeDecl, head_name: Option<&Arc<str>>, head_uuid: Option<Uuid>) -> bool {
    let uuid_match = matches!((head_uuid, decl.uuid), (Some(h), Some(d)) if h == d);
    let name_match = head_name.is_some_and(|hn| *hn == decl.name);
    uuid_match || name_match
}
