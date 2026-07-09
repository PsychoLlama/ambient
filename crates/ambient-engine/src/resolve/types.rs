//! Canonicalizing type references: rewriting qualified type spellings
//! inside `types::Type` values to the types they name.

use std::sync::Arc;

use crate::ast::ItemKind;
use crate::module_path::ModulePath;
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
                let name = Arc::clone(&n.name);
                if !name.contains("::") {
                    return;
                }
                let segments: Vec<Arc<str>> = name.split("::").map(Arc::from).collect();
                let Some((item, prefix)) = segments.split_last() else {
                    return;
                };
                let Some(target) = self.resolve_module_prefix(prefix) else {
                    return;
                };
                if target == *self.current {
                    // A self-reference by qualified path: the bare local
                    // name is the canonical spelling.
                    if let Type::Named(n) = ty {
                        n.name = Arc::clone(item);
                    }
                    return;
                }
                // Visibility check (and re-export chasing) through the
                // ordinary symbol lookup.
                let Ok((_, origin)) = self.registry.lookup_symbol(&target, item) else {
                    return;
                };
                self.deps.insert(self.registry.module_id(&origin));
                let item = Arc::clone(item);
                self.apply_named_type_from_module(ty, &origin, &item);
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

    /// Rewrite the `Type::Named` head at `ty` to the type named by `item`
    /// in its defining `module`, given the identity `module` already
    /// determined. This is the single rewrite entry point for both the
    /// qualified spelling (`pkg.shapes.Money`) and — from Phase 2 — a bare
    /// same-scope spelling:
    ///
    /// - an enum → `Named` with the enum's bare name and nominal uuid
    ///   (exactly what the enum's own constructors produce),
    /// - a non-generic struct or type alias → the aliased type itself
    ///   (`unique` aliases are already wrapped in `Type::Nominal`, so
    ///   identity rides along),
    /// - an opaque generic struct (`extern unique(…) struct List<T>;`) →
    ///   `Named` with the bare name, the written arguments, and the
    ///   declaration's uuid — the same applied form the checker builds for
    ///   an in-scope bare spelling.
    ///
    /// Generic *fielded* structs and generic type aliases need parameter
    /// substitution, which belongs to the checker; they are left bare and
    /// the checker substitutes.
    pub(super) fn apply_named_type_from_module(
        &mut self,
        ty: &mut Type,
        module: &ModulePath,
        item: &str,
    ) {
        let Some(info) = self.registry.get(module) else {
            return;
        };
        for decl in &info.module.items {
            match &decl.kind {
                ItemKind::Enum(def) if def.name.as_ref() == item => {
                    let uuid = def.uuid;
                    let name = Arc::clone(&def.name);
                    if let Type::Named(n) = ty {
                        *ty = Type::Named(NamedType {
                            name,
                            args: std::mem::take(&mut n.args),
                            uuid: Some(uuid),
                        });
                    }
                    return;
                }
                ItemKind::Struct(def)
                    if def.name.as_ref() == item && def.type_params.is_empty() =>
                {
                    *ty = def.ty.clone();
                    return;
                }
                // An opaque generic head: identity plus the written
                // arguments (phantom parameters, nothing to substitute).
                // Mirrors `AliasTarget::of_struct`.
                ItemKind::Struct(def)
                    if def.name.as_ref() == item
                        && def.is_extern
                        && def.is_unit()
                        && def.unique_id.is_some() =>
                {
                    let uuid = def.unique_id;
                    let name = Arc::clone(&def.name);
                    if let Type::Named(n) = ty {
                        *ty = Type::Named(NamedType {
                            name,
                            args: std::mem::take(&mut n.args),
                            uuid,
                        });
                    }
                    return;
                }
                ItemKind::TypeAlias(def)
                    if def.name.as_ref() == item && def.type_params.is_empty() =>
                {
                    *ty = def.ty.clone();
                    return;
                }
                _ => {}
            }
        }
    }
}
