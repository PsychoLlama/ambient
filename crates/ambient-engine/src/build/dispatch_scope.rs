//! Per-module dispatch-shape narrowing (Phase 5 extension).
//!
//! The build-global [`dispatch_surface_hash`](crate::module_interface::dispatch_surface_hash)
//! folds *every* impl in the package into *every* module's cache key, so adding
//! an impl (or changing an impl signature) anywhere invalidated every module —
//! even ones that can never dispatch it. This module narrows that channel: each
//! module's key folds only the impl shapes it *could* dispatch, so an impl add
//! on one package type spares modules that never touch that type.
//!
//! # The soundness backbone: dispatch requires holding the type
//!
//! A module's compiled objects and diagnostics depend on a foreign impl only if
//! the module could *dispatch* it (a method call, operator, or associated call,
//! or instantiating a bound at a concrete type). Dispatch compiles to a
//! content-addressed `<type-uuid>::<trait-uuid>::method` symbol, and — verified
//! against the compiler — the module that hard-links that symbol always
//! **names or holds the concrete receiver type**: a direct method call on a
//! concrete value, or a call site that instantiates a trait bound at a concrete
//! type (`DictSource::Impl`). A purely-generic function that only forwards its
//! dictionary (`DictSource::Param`) links nothing concrete. So:
//!
//! > If module `M` can dispatch an impl for type `T`, then `M` holds a `T`, and
//! > to hold a `T` its source must obtain one — transitively bottoming out at
//! > `T`'s definition. Hence `M` transitively resolve-depends on `T`'s defining
//! > module.
//!
//! This is the same structural fact [`super::reachability`] uses. So a module's
//! **relevant** impls are those whose target type is declared in a module the
//! module transitively resolve-depends on (or itself). Operators need no special
//! case: `a + b` dispatches on the *receiver type*, which is what we key on —
//! the reserved `Add` trait uuid never has to be recognized.
//!
//! # What stays global
//!
//! Two classes of impl cannot be pinned to a package dependency and stay folded
//! into *every* module's key (the `global` bytes):
//!
//! - **Unconditional impls** — an impl on a builtin/prelude/core type (every
//!   module can hold one, via the prelude, with no package dep) or a
//!   blanket/param impl (`impl<T> Show for T`, dispatchable on anything). Core
//!   and platform impls also land here: their target type is not a *package*
//!   module, so they are treated as unconditional (and package modules that
//!   dispatch them import the defining core module directly, so the dependency
//!   `interface_hash` channel covers their body/signature changes too).
//!
//! - **Colliding impls** — coherence is checked build-globally: every module's
//!   type-check seeds a fresh registry with *all* package impls and independently
//!   reports a duplicate `impl Trait for T`. A cold check therefore reports the
//!   collision in *every* module's diagnostics, so a duplicate add must
//!   invalidate every module. We detect collisions structurally (two impls
//!   sharing a `(trait, type-head)`, or two inherent impls sharing a
//!   `(type-head, method)`) and promote *both* participants to the global bytes.
//!   A *unique* impl add creates no collision, so the global bytes are unchanged
//!   and unrelated modules stay cached — the narrowing win — while a duplicate
//!   add moves the global bytes and re-checks the package, re-surfacing the
//!   error. (The two colliding impls both name the shared type, so they also
//!   re-check via their own per-module relevance; the global promotion is what
//!   keeps the error surfacing in *non-holders*, matching cold.)
//!
//! # Why abilities are absent entirely — not global, not narrowed
//!
//! Abilities carry **no** dispatch-key input, global or per-module: the
//! dependency `interface_hash` channel already covers every consumer, so folding
//! ability shapes here would be redundant double-counting.
//!
//! The reason abilities differ from impls is that they have no *orphans*. An
//! impl can live in a third module the dispatcher never imports (an orphan `Add
//! for Widget`), so type-directed dispatch reaches it with no resolve-dep edge —
//! which is exactly why impls need the type-anchored narrowing (or the global
//! bucket). An ability cannot be consumed that way: **every** path that makes a
//! module's check or objects depend on ability `A`'s *shape* spells `A`, and the
//! resolve pass canonicalizes every spelled ability reference to `A`'s declaring
//! module (`resolve::refs::resolve_ability_ref`), recording a direct resolve-dep
//! edge:
//!
//! - a perform site (`A::method!(..)`), a handler arm / `handle` expression, a
//!   `Handler<A, R>` annotation, and an ability-method bound all name `A`;
//! - an `A` effect row on a signature names `A` (and `pub` items must annotate
//!   their effect row, so a cross-module-visible propagator spells it);
//! - the prelude's `Exception` is no exception: an unqualified `throw`
//!   canonicalizes to `core::exception`, its *declaring* module, not the prelude.
//!
//! The one path that does *not* spell `A` — a module that merely calls a
//! function performing `A` and re-propagates the effect without handling it —
//! does not depend on `A`'s shape at all: it never dispatches an `A` method, so
//! adding/removing an `A` method or flipping a never-flag cannot change its
//! objects or diagnostics. It depends only on the *callee's* signature (which
//! carries the `A` effect identity), covered by the callee module's dep edge.
//!
//! So a direct resolve-dep edge to `A`'s module exists for exactly the modules
//! whose output depends on `A`'s shape, and [`dep_interface_hashes`] folds that
//! module's full `interface_hash` — which *retains* the ability's signature and
//! default-impl body hash — into their cache key. A signature/never-flag edit,
//! or a method add/remove, moves the declaring module's `interface_hash` and
//! re-checks its consumers; unrelated modules stay warm. A default-impl *body*
//! edit is covered identically to impl bodies: the dispatch surface has always
//! been body-free, so consumers relink/recompile through the ordinary
//! callee-hash channels (link validation for the build cache, the retained-body
//! `interface_hash` for importers), never a whole-package re-check.
//!
//! [`dep_interface_hashes`]: super::dep_interface_hashes
//!
//! # Determinism
//!
//! Every fold sorts its shape-byte vectors, so the result is independent of
//! module/item iteration order.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ast::{ItemKind, Module};
use crate::module_interface::impl_dispatch_shape;
use crate::module_registry::ModuleRegistry;

use super::reachability::{TypeDecl, declared_types, forward_closure, type_head, type_matches};

/// Domain separator for the global (unconditional + colliding impl) bytes.
/// Abilities carry no dispatch-key input (the dependency channel covers them);
/// the version is bumped from `v1` so a stale cache from the ability-global era
/// misses cleanly rather than reusing a key that folded ability shapes.
const GLOBAL_VERSION: &[u8] = b"ambient/dispatch/global/v2";
/// Domain separator for a module's narrowed dispatch key.
const MODULE_VERSION: &[u8] = b"ambient/dispatch/permodule/v1";

/// A raw impl block plus its defining module, gathered before routing/collision
/// analysis (which needs every impl's grouping key first).
struct Raw<'a> {
    module_id: &'a str,
    imp: &'a crate::ast::ImplDef,
}

/// One impl block's routing and shape, derived from a resolved (unchecked) AST.
struct ImplFacts {
    /// The package module(s) that declare the target type — the modules a
    /// dispatcher must transitively depend on to hold the type. Empty means
    /// *unconditional* (a builtin/prelude/core type or a blanket/param impl):
    /// no package dependency can rule out a dispatcher.
    package_anchors: Vec<String>,
    /// The body-free dispatch-shape bytes ([`impl_dispatch_shape`]).
    shape: Vec<u8>,
    /// Whether this impl participates in a coherence collision (promoted to the
    /// global bytes so a duplicate re-checks the whole package).
    colliding: bool,
}

/// Compute each package module's narrowed dispatch-key input, replacing the
/// single build-global [`dispatch_surface_hash`](crate::module_interface::dispatch_surface_hash)
/// in the per-module cache key. Returned map is keyed by canonical module
/// identity (matching `dep_ids` / the interface map); modules absent from the
/// result (none, in practice) fall back to a fresh key input.
///
/// `dep_ids` is the resolve-pass dependency graph keyed by canonical module
/// identity — package modules are its keys; values may name core/platform
/// modules (ignored: they are always present, so their impls are unconditional).
#[must_use]
pub fn per_module_dispatch_hashes(
    registry: &ModuleRegistry,
    dep_ids: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, [u8; 32]> {
    let modules: Vec<(String, &Module)> = registry
        .all_modules()
        .map(|info| (registry.module_id(&info.path).to_string(), &*info.module))
        .collect();
    let declared: HashMap<&str, Vec<TypeDecl>> = modules
        .iter()
        .map(|(id, ast)| (id.as_str(), declared_types(ast)))
        .collect();

    let facts = collect_impl_facts(&modules, dep_ids, &declared);

    // The global bytes: unconditional + colliding impl shapes, folded into every
    // module's key. Abilities are deliberately absent — the dependency
    // `interface_hash` channel already covers every performer/handler (see the
    // module docs), so folding ability shapes here would double-count.
    let mut global_shapes: Vec<&[u8]> = Vec::new();
    for f in &facts {
        if f.colliding || f.package_anchors.is_empty() {
            global_shapes.push(&f.shape);
        }
    }
    global_shapes.sort_unstable();
    let mut gh = blake3::Hasher::new();
    gh.update(GLOBAL_VERSION);
    fold_shapes(&mut gh, &global_shapes);
    let global = gh.finalize();

    // Per module: the global bytes plus the shapes of the non-global impls
    // (package-anchored, non-colliding) whose target type this module can hold.
    let mut out = BTreeMap::new();
    for (id, _) in &modules {
        // Core/platform modules never key against this channel (they are
        // compiled as one cache unit and never dispatch user types); only
        // package modules — the `dep_ids` keys — get a narrowed input.
        if !dep_ids.contains_key(id) {
            continue;
        }
        let mut reach = forward_closure(dep_ids, id);
        reach.insert(id.clone());

        let mut relevant: Vec<&[u8]> = Vec::new();
        for f in &facts {
            if f.colliding || f.package_anchors.is_empty() {
                continue; // already in the global bytes
            }
            if f.package_anchors.iter().any(|a| reach.contains(a)) {
                relevant.push(&f.shape);
            }
        }
        relevant.sort_unstable();

        let mut h = blake3::Hasher::new();
        h.update(MODULE_VERSION);
        h.update(global.as_bytes());
        fold_shapes(&mut h, &relevant);
        out.insert(id.clone(), *h.finalize().as_bytes());
    }
    out
}

/// Walk every module's impl blocks, resolving each to its routing + shape and
/// flagging coherence collisions.
fn collect_impl_facts(
    modules: &[(String, &Module)],
    dep_ids: &BTreeMap<String, Vec<String>>,
    declared: &HashMap<&str, Vec<TypeDecl>>,
) -> Vec<ImplFacts> {
    // Coherence grouping, computed first so `colliding` is known per impl:
    // trait impls by `(trait-ref, type-head)`, inherent impls by
    // `(type-head, method)`. Any key with >1 member marks its impls colliding.
    let mut trait_groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    let mut inherent_groups: HashMap<(String, String), Vec<usize>> = HashMap::new();

    let mut raws: Vec<Raw<'_>> = Vec::new();
    for (module_id, ast) in modules {
        for item in &ast.items {
            let ItemKind::Impl(imp) = &item.kind else {
                continue;
            };
            let head = type_head(&imp.for_type).map(|(name, uuid)| head_key(name.as_ref(), uuid));
            let idx = raws.len();
            if let Some(head) = &head {
                match &imp.trait_name {
                    Some(tr) => {
                        let key = (crate::module_interface::render_name(tr), head.clone());
                        trait_groups.entry(key).or_default().push(idx);
                    }
                    None => {
                        for m in &imp.methods {
                            inherent_groups
                                .entry((head.clone(), m.name.to_string()))
                                .or_default()
                                .push(idx);
                        }
                    }
                }
            }
            raws.push(Raw { module_id, imp });
        }
    }

    let mut colliding: BTreeSet<usize> = BTreeSet::new();
    for members in trait_groups.values().chain(inherent_groups.values()) {
        if members.len() > 1 {
            colliding.extend(members.iter().copied());
        }
    }

    raws.iter()
        .enumerate()
        .map(|(idx, raw)| {
            let package_anchors = package_anchors_of(raw.imp, raw.module_id, dep_ids, declared);
            ImplFacts {
                package_anchors,
                shape: impl_dispatch_shape(raw.imp),
                colliding: colliding.contains(&idx),
            }
        })
        .collect()
}

/// The package module(s) declaring an impl's target type — the modules a
/// dispatcher must transitively depend on to hold it. Empty for a blanket/param
/// impl or a target that is not a *package* type (builtin/prelude/core), which
/// is unconditional. Mirrors [`super::reachability`]'s anchoring: search only the
/// impl's own module plus its direct resolve deps (where a named type must be
/// defined), so a same-named type elsewhere cannot manufacture a false anchor.
fn package_anchors_of(
    imp: &crate::ast::ImplDef,
    module_id: &str,
    dep_ids: &BTreeMap<String, Vec<String>>,
    declared: &HashMap<&str, Vec<TypeDecl>>,
) -> Vec<String> {
    let Some((head_name, head_uuid)) = type_head(&imp.for_type) else {
        return Vec::new(); // blanket/param impl: unconditional
    };
    let mut scope: Vec<&str> = dep_ids
        .get(module_id)
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    scope.push(module_id);
    scope
        .into_iter()
        // Only *package* modules can anchor a narrowing; a core/platform target
        // is unconditional (and covered by the dependency channel for importers).
        .filter(|d| dep_ids.contains_key(*d))
        .filter(|d| {
            declared.get(d).is_some_and(|types| {
                types
                    .iter()
                    .any(|t| type_matches(t, head_name.as_ref(), head_uuid))
            })
        })
        .map(ToString::to_string)
        .collect()
}

/// A stable string identity for a type head: its uuid when present (the strict
/// nominal identity), else its name. Grouping by *head* (not the full rendered
/// type) is deliberate — `impl<T> Eq for Pair<T>` and `impl Eq for Pair<Number>`
/// share the head `Pair` and collide, exactly as the checker's coherence key
/// does, so collision detection never *under*-groups.
fn head_key(name: Option<&std::sync::Arc<str>>, uuid: Option<uuid::Uuid>) -> String {
    if let Some(u) = uuid {
        return u.to_string();
    }
    name.map(ToString::to_string).unwrap_or_default()
}

/// Fold a sorted list of shape-byte slices into a hasher, length-prefixed so
/// concatenation is unambiguous.
#[allow(clippy::cast_possible_truncation)]
fn fold_shapes(h: &mut blake3::Hasher, shapes: &[&[u8]]) {
    h.update(&(shapes.len() as u32).to_le_bytes());
    for s in shapes {
        h.update(&(s.len() as u32).to_le_bytes());
        h.update(s);
    }
}
