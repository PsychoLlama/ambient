//! Trait/impl member queries: what an `impl` block still has to provide.
//!
//! The engine's checker is the authority on impl completeness
//! (`infer/check/impls.rs` reports missing methods and unbound associated
//! types); this module answers the *editor's* version of the same question —
//! "which members could be inserted here" — from the same AST facts, resolving
//! the implemented trait through the registry exactly like go-to-definition.

use std::sync::Arc;

use ambient_engine::ast::{
    ImplDef, ItemKind, Module, QualifiedName, Span, TraitAssocType, TraitDef, TraitMethod,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use super::{item_name_span, resolve_qualified_name};

/// The members a trait impl hasn't provided yet: unimplemented methods and
/// unbound associated types, cloned from the trait's declaration AST.
#[derive(Debug)]
pub struct MissingImplMembers {
    /// The implemented trait's name, as declared.
    pub trait_name: Arc<str>,
    /// The span of the whole impl item (for frontends to locate its body).
    pub impl_span: Span,
    /// Trait methods with no same-named implementation in the impl.
    pub methods: Vec<TraitMethod>,
    /// Declared associated types with no same-named binding in the impl.
    pub assoc_types: Vec<TraitAssocType>,
}

/// The trait members still missing from the impl block whose *item position*
/// contains `offset` — inside the impl's span but not inside any of its
/// methods or associated-type bindings. `None` for inherent impls, offsets
/// outside any impl, or a trait that doesn't resolve.
#[must_use]
pub fn missing_impl_members_at_offset(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    offset: u32,
) -> Option<MissingImplMembers> {
    let (impl_span, impl_def) = impl_at_item_position(module, offset)?;
    let trait_ref = impl_def.trait_name.as_ref()?;
    let trait_def = resolve_trait_def(module, module_path, registry, &trait_ref.name)?;

    let methods = trait_def
        .methods
        .iter()
        .filter(|m| !impl_def.methods.iter().any(|im| im.name == m.name))
        .cloned()
        .collect();
    let assoc_types = trait_def
        .assoc_types
        .iter()
        .filter(|a| !impl_def.assoc_types.iter().any(|ia| ia.name == a.name))
        .cloned()
        .collect();

    Some(MissingImplMembers {
        trait_name: Arc::clone(&trait_def.name),
        impl_span,
        methods,
        assoc_types,
    })
}

/// The impl block containing `offset` at item position: within the impl's
/// span but not within any member's span (a cursor inside a method body gets
/// expression completions, not member stubs).
fn impl_at_item_position(module: &Module, offset: u32) -> Option<(Span, &ImplDef)> {
    let contains = |span: &Span| offset >= span.start && offset <= span.end;
    module.items.iter().find_map(|item| {
        let ItemKind::Impl(impl_def) = &item.kind else {
            return None;
        };
        if !contains(&item.span)
            || impl_def.methods.iter().any(|m| contains(&m.span))
            || impl_def.assoc_types.iter().any(|a| contains(&a.span))
        {
            return None;
        }
        Some((item.span, impl_def))
    })
}

/// Resolve a trait reference to its declaring [`TraitDef`] AST — local or
/// cross-module — through the same registry resolution definitions use.
fn resolve_trait_def<'a>(
    module: &'a Module,
    module_path: &ModulePath,
    registry: &'a ModuleRegistry,
    qname: &QualifiedName,
) -> Option<&'a TraitDef> {
    let def = resolve_qualified_name(module, module_path, registry, qname)?;
    let origin = match &def.module {
        Some(path) => &registry.get(path)?.module,
        None => module,
    };
    origin.items.iter().find_map(|item| match &item.kind {
        ItemKind::Trait(t) if item_name_span(item) == Some(def.span) => Some(t),
        _ => None,
    })
}
