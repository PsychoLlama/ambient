//! Types as a core tree holds them. See `spec/core.md`.
//!
//! A core tree names another definition by a slot, so whoever lowers a
//! type says which slot a symbol has.

use super::{Env, Ty, TyKey, abilities};
use crate::core::{self, Slot};
use crate::surface::{Key, Reference};
use crate::symbol::Symbol;

/// The slot a symbol has in the table of the definition being made.
pub type Slots<'a> = &'a mut dyn FnMut(&Symbol) -> Slot;

impl Env {
    /// The entries of a record type in the order a core tree holds them:
    /// fields by name, then what is keyed by a definition, by its tag. Neither
    /// depends on what anything is called.
    #[must_use]
    pub fn ordered<'t>(&self, fields: &'t [(TyKey, Ty)]) -> Vec<(&'t TyKey, &'t Ty)> {
        let handler = |key: &TyKey| matches!(key, Key::Ability(_));
        let identity = |key: &TyKey| match key {
            Key::Ability(Reference::Definition(named)) => {
                let ability = self.abilities.get(named).map(|info| &info.identity);
                ability.or_else(|| self.keys.get(named).map(|info| &info.identity))
            }
            _ => None,
        };
        let mut entries: Vec<_> = fields.iter().map(|(key, ty)| (key, ty)).collect();
        entries.sort_by(|(a, _), (b, _)| {
            (handler(a), identity(a))
                .cmp(&(handler(b), identity(b)))
                .then_with(|| a.cmp(b))
        });
        entries
    }

    /// A row ordered by identity, each ability once: the one order that
    /// depends on neither names nor where the row was written.
    #[must_use]
    pub fn row(&self, row: &[Symbol]) -> Vec<Symbol> {
        let identity = |ability: &Symbol| self.abilities.get(ability).map(|info| &info.identity);
        let mut row = row.to_vec();
        row.sort_by(|a, b| identity(a).cmp(&identity(b)).then_with(|| a.cmp(b)));
        row.dedup();
        row
    }

    /// A key as a core tree holds it.
    pub fn lower_key(&self, key: &TyKey, slot: Slots<'_>) -> core::Key {
        match key {
            Key::Ability(Reference::Definition(ability)) => core::Key::Ability(slot(ability)),
            Key::Field(name) => core::Key::Field(name.clone()),
            // Resolution only ever names an ability here.
            Key::Ability(_) => core::Key::Field(String::new()),
        }
    }

    /// A normalized type as a core type.
    pub fn lower(&self, ty: &Ty, slot: Slots<'_>) -> core::Type {
        let mut all = |types: &[Ty]| -> Vec<core::Type> {
            types.iter().map(|ty| self.lower(ty, slot)).collect()
        };
        match ty {
            Ty::Never => core::Type::Never,
            Ty::Unknown => core::Type::Unknown,
            // Resolution never produces a local in a type.
            Ty::Null | Ty::Named(Reference::Local(_), _) => core::Type::Null,
            Ty::Named(Reference::Parameter(position), _) => core::Type::Parameter(*position),
            Ty::Bool => core::Type::Bool,
            Ty::Int => core::Type::Int,
            Ty::Float => core::Type::Float,
            Ty::String => core::Type::String,
            Ty::Bytes => core::Type::Bytes,
            Ty::Tag => core::Type::Tag,
            Ty::Literal(literal) => core::Type::Literal(literal.clone()),
            Ty::Brand(brand) => core::Type::Brand(brand.clone()),
            Ty::Branded(ty, brands) => {
                let brand = |brand: &Ty| match brand {
                    Ty::Brand(brand) => Some(brand.clone()),
                    _ => None,
                };
                let brands = brands.iter().filter_map(brand).collect();
                core::Type::Branded(Box::new(self.lower(ty, slot)), brands)
            }
            Ty::Record(fields) => core::Type::Record(
                self.ordered(fields)
                    .into_iter()
                    .map(|(key, ty)| (self.lower_key(key, slot), self.lower(ty, slot)))
                    .collect(),
            ),
            Ty::Tuple(members) => core::Type::Tuple(all(members)),
            Ty::List(member) => core::Type::List(Box::new(self.lower(member, slot))),
            Ty::Cell(member) => core::Type::Cell(Box::new(self.lower(member, slot))),
            Ty::Iterator(member) => core::Type::Iterator(Box::new(self.lower(member, slot))),
            Ty::Method { needs, function } => core::Type::Method {
                needs: Box::new(self.lower(needs, slot)),
                function: Box::new(self.lower(function, slot)),
            },
            Ty::Function { params, ret, row } => core::Type::Function {
                params: all(params),
                ret: Box::new(self.lower(ret, slot)),
                row: self.row(&abilities(row)).iter().map(slot).collect(),
            },
            Ty::Bound(index) => core::Type::Bound(*index),
            Ty::Generic { bounds, function } => core::Type::Generic {
                bounds: all(bounds),
                function: Box::new(self.lower(function, slot)),
            },
            Ty::Union(members) => {
                let mut members = all(members);
                members.sort_by_key(core::Type::encode);
                members.dedup();
                core::Type::Union(members)
            }
            Ty::Named(Reference::Definition(symbol), args) => {
                let args = all(args);
                core::Type::Reference(slot(symbol), args)
            }
        }
    }
}
