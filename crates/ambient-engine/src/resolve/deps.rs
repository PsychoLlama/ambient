//! The classified dependency-recording funnel.
//!
//! The value/type dep split is the single point of truth for compile
//! ordering: a value-position reference recorded *without* being classified
//! as such silently drops a link-ordering edge, which surfaces as a link
//! failure at build time. To make that misclassification structurally
//! impossible rather than merely reviewable, every dep edge the resolve pass
//! learns is recorded through [`DepRecorder::record`] — the sole mutator —
//! and the two underlying sets are private to this module, so no call site
//! anywhere else can reach past the classifier and write a set directly.
//!
//! `record` takes a [`RefPos`], and that is the *only* thing that decides
//! link membership: a [`RefPos::Value`] edge lands in both `deps` and
//! `link_deps`; every other position ([`RefPos::Type`], [`RefPos::Import`])
//! lands in `deps` alone. `link_deps ⊆ deps` therefore holds by
//! construction — `record` is the single writer of both sets and never
//! writes `link_deps` without also writing `deps`.

use std::collections::BTreeSet;

use crate::fqn::ModuleId;

use super::RefPos;

/// The resolve pass's accumulated dependency sets, mutable only through the
/// classified [`Self::record`] funnel. The fields are module-private on
/// purpose: it makes "a value-position edge always reaches `link_deps`, a
/// type/import edge never does" a structural invariant, not a convention a
/// reviewer must police at every `insert`.
#[derive(Default)]
pub(super) struct DepRecorder {
    /// Foreign modules that references resolved into — the full superset
    /// (value *and* type references, plus `use` imports). See
    /// [`ResolveOutcome::deps`](super::ResolveOutcome::deps).
    deps: BTreeSet<ModuleId>,
    /// The link-order subset of [`Self::deps`]: only [`RefPos::Value`]
    /// positions. See [`ResolveOutcome::link_deps`](super::ResolveOutcome::link_deps).
    link_deps: BTreeSet<ModuleId>,
}

impl DepRecorder {
    /// Record `module` as a dependency, classified by `pos`. The single
    /// dep-recording entry point of the whole resolve pass: a
    /// [`RefPos::Value`] edge (a call/const ref, variant or unit-struct
    /// construction, ability perform/handler, module-alias method call)
    /// writes **both** sets; a [`RefPos::Type`] edge (a typed-record
    /// construction or a qualified type path — the compiler emits no link
    /// artifact for either) or a [`RefPos::Import`] edge (a `use` statement)
    /// writes `deps` **only**. Keeping `link_deps` a strict subset of `deps`
    /// is therefore automatic: this method never writes `link_deps` without
    /// also writing `deps`.
    ///
    /// Callers guard against self-edges before calling (the resolve pass
    /// never depends on its own module); this method records whatever it is
    /// handed.
    pub(super) fn record(&mut self, module: &ModuleId, pos: RefPos) {
        self.deps.insert(module.clone());
        if pos == RefPos::Value {
            self.link_deps.insert(module.clone());
        }
    }

    /// Consume the recorder into its `(deps, link_deps)` sets for
    /// [`ResolveOutcome`](super::ResolveOutcome).
    pub(super) fn into_parts(self) -> (BTreeSet<ModuleId>, BTreeSet<ModuleId>) {
        (self.deps, self.link_deps)
    }
}
