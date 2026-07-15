//! Trait/impl member queries: what an `impl` block still has to provide.
//!
//! The engine's checker is the authority on impl completeness
//! (`infer/check/impls.rs` reports missing methods and unbound associated
//! types); this module answers the *editor's* version of the same question —
//! "which members could be inserted here" — from the same AST facts, resolving
//! the implemented trait through the registry exactly like go-to-definition.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::ast::{
    ImplDef, ItemKind, Module, QualifiedName, Span, TraitAssocType, TraitDef, TraitMethod,
};
use ambient_engine::infer::inherent::{ImplKey, impl_key_for};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::types::Type;

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

/// A member reachable through `.` on a receiver of a known type.
#[derive(Debug)]
pub enum ReceiverMember {
    /// A record field of the receiver's (possibly nominal) record body.
    Field { name: Arc<str>, ty: Type },
    /// A `self`-taking method from an inherent or trait impl of the
    /// receiver's nominal identity.
    Method {
        name: Arc<str>,
        /// Parameter names and declared types, excluding `self`.
        params: Vec<(Arc<str>, Option<Type>)>,
        ret_ty: Option<Type>,
        /// The implemented trait's spelled name; `None` for an inherent method.
        trait_name: Option<Arc<str>>,
    },
}

impl ReceiverMember {
    /// The member's name — the completion label.
    #[must_use]
    pub fn name(&self) -> &Arc<str> {
        match self {
            Self::Field { name, .. } | Self::Method { name, .. } => name,
        }
    }
}

/// The checker-inferred type of the innermost expression at `receiver_end` —
/// the offset of the receiver's final character, one before the `.` being
/// completed.
#[must_use]
pub fn receiver_type_at(module: &Module, receiver_end: u32) -> Option<Type> {
    super::find_expr_at_offset(module, receiver_end).and_then(|expr| expr.ty.clone())
}

/// Every `.`-reachable member of `ty`: its record fields, then the
/// `self`-taking methods of every impl of its nominal identity, from the
/// current module's AST and every registry module's. Matching uses
/// [`impl_key_for`] — the same identity the checker dispatches on — so
/// completion can't disagree with method resolution. Inherent methods shadow
/// same-named trait methods (the checker's resolution order); scope is not
/// consulted (an unimported trait's method may be over-offered, never a
/// missing one). Fields come first, then methods sorted by name.
#[must_use]
pub fn receiver_members(
    ty: &Type,
    module: &Module,
    registry: &ModuleRegistry,
) -> Vec<ReceiverMember> {
    let mut members = Vec::new();

    let record = match ty {
        Type::Record(rec) => Some(rec),
        Type::Nominal(nom) => match nom.inner.as_ref() {
            Type::Record(rec) => Some(rec),
            _ => None,
        },
        _ => None,
    };
    if let Some(rec) = record {
        for (name, field_ty) in &rec.fields {
            members.push(ReceiverMember::Field {
                name: Arc::clone(name),
                ty: field_ty.clone(),
            });
        }
    }

    let Some(key) = impl_key_for(ty) else {
        return members;
    };
    // name → (is_inherent, member); the module registry also holds the
    // current module, so same-named duplicates are expected and collapse here.
    let mut methods: HashMap<Arc<str>, (bool, ReceiverMember)> = HashMap::new();
    collect_impl_methods(module, &key, &mut methods);
    for info in registry.all_modules() {
        collect_impl_methods(&info.module, &key, &mut methods);
    }
    let mut methods: Vec<_> = methods.into_values().map(|(_, m)| m).collect();
    methods.sort_by(|a, b| a.name().cmp(b.name()));
    members.extend(methods);
    members
}

/// Collect `module`'s impl methods targeting `key` into `out`, letting an
/// inherent method displace a same-named trait method but never the reverse.
fn collect_impl_methods(
    module: &Module,
    key: &ImplKey,
    out: &mut HashMap<Arc<str>, (bool, ReceiverMember)>,
) {
    for item in &module.items {
        let ItemKind::Impl(impl_def) = &item.kind else {
            continue;
        };
        if impl_key_for(&impl_def.for_type).as_ref() != Some(key) {
            continue;
        }
        let trait_name = impl_def
            .trait_name
            .as_ref()
            .map(|t| Arc::clone(&t.name.name));
        for method in &impl_def.methods {
            if !method.has_self {
                continue;
            }
            let inherent = trait_name.is_none();
            let entry = (
                inherent,
                ReceiverMember::Method {
                    name: Arc::clone(&method.name),
                    params: method
                        .params
                        .iter()
                        .map(|p| (Arc::clone(&p.name), p.ty.clone()))
                        .collect(),
                    ret_ty: method.ret_ty.clone(),
                    trait_name: trait_name.clone(),
                },
            );
            match out.entry(Arc::clone(&method.name)) {
                std::collections::hash_map::Entry::Occupied(mut existing) => {
                    if inherent && !existing.get().0 {
                        existing.insert(entry);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
            }
        }
    }
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
