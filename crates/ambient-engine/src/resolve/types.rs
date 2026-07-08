//! Canonicalizing type references: rewriting qualified type spellings
//! inside `types::Type` values to the types they name.

use std::sync::Arc;

use crate::ast::ItemKind;
use crate::types::{NamedType, Type};

use super::Resolver;

impl Resolver<'_> {
    /// Resolve a dotted type reference inside a `types::Type` value.
    ///
    /// Type syntax lowers qualified names to dotted `Named` heads
    /// (`pkg.shapes.Money`, `types.Shape`); this rewrites each to the
    /// type it names:
    ///
    /// - an enum → `Named` with the enum's bare name and nominal uuid
    ///   (exactly what the enum's own constructors produce),
    /// - a non-generic struct or type alias → the aliased type itself
    ///   (`unique` aliases are already wrapped in `Type::Nominal`, so
    ///   identity rides along),
    /// - an opaque generic struct (`extern unique(…) struct List<T>;`) →
    ///   `Named` with the bare name, the written arguments, and the
    ///   declaration's uuid — the same applied form the checker builds for
    ///   an in-scope bare spelling, so `core::collections::List<T>` and a
    ///   prelude `List<T>` are one type,
    /// - a reference into the current module → the bare local spelling.
    ///
    /// Generic *fielded* structs and generic type aliases would need
    /// parameter substitution, which belongs to the checker; qualified
    /// references to them stay unresolved and surface as undefined-type
    /// errors for now.
    pub(super) fn resolve_type(&mut self, ty: &mut Type) {
        match ty {
            Type::Named(n) => {
                for arg in &mut n.args {
                    self.resolve_type(arg);
                }
                if !n.name.contains("::") {
                    return;
                }
                let segments: Vec<Arc<str>> = n.name.split("::").map(Arc::from).collect();
                let Some((item, prefix)) = segments.split_last() else {
                    return;
                };
                let Some(target) = self.resolve_module_prefix(prefix) else {
                    return;
                };
                if target == *self.current {
                    // A self-reference by qualified path: the bare local
                    // name is the canonical spelling.
                    n.name = Arc::clone(item);
                    return;
                }
                // Visibility check (and re-export chasing) through the
                // ordinary symbol lookup.
                let Ok((_, origin)) = self.registry.lookup_symbol(&target, item) else {
                    return;
                };
                let Some(info) = self.registry.get(&origin) else {
                    return;
                };
                self.deps.insert(self.registry.module_id(&origin));
                for decl in &info.module.items {
                    match &decl.kind {
                        ItemKind::Enum(def) if def.name == *item => {
                            *ty = Type::Named(NamedType {
                                name: Arc::clone(&def.name),
                                args: std::mem::take(&mut n.args),
                                uuid: Some(def.uuid),
                            });
                            return;
                        }
                        ItemKind::Struct(def)
                            if def.name == *item && def.type_params.is_empty() =>
                        {
                            *ty = def.ty.clone();
                            return;
                        }
                        // An opaque generic head: identity plus the written
                        // arguments (phantom parameters, nothing to
                        // substitute). Mirrors `AliasTarget::of_struct`.
                        ItemKind::Struct(def)
                            if def.name == *item
                                && def.is_extern
                                && def.is_unit()
                                && def.unique_id.is_some() =>
                        {
                            *ty = Type::Named(NamedType {
                                name: Arc::clone(&def.name),
                                args: std::mem::take(&mut n.args),
                                uuid: def.unique_id,
                            });
                            return;
                        }
                        ItemKind::TypeAlias(def)
                            if def.name == *item && def.type_params.is_empty() =>
                        {
                            *ty = def.ty.clone();
                            return;
                        }
                        _ => {}
                    }
                }
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.resolve_type(elem);
                }
            }
            Type::Record(rec) => {
                let fields = std::mem::take(&mut rec.fields);
                rec.fields = fields
                    .into_iter()
                    .map(|(name, mut field_ty)| {
                        self.resolve_type(&mut field_ty);
                        (name, field_ty)
                    })
                    .collect();
            }
            Type::Function(f) => {
                for param in &mut f.params {
                    self.resolve_type(param);
                }
                self.resolve_type(&mut f.ret);
            }
            Type::Nominal(n) => self.resolve_type(&mut n.inner),
            Type::Forall(forall) => self.resolve_type(&mut forall.body),
            _ => {}
        }
    }
}
