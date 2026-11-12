//! Types in a core tree: what one means, and whether it is well-formed
//! and written in the one form it has.

use super::{Invalid, Verifier};
use crate::core::{self, Slot};
use crate::surface::{Key, Reference};
use crate::symbol::Symbol;
use crate::types::Ty;

/// A core type as the relations between types read one: each slot
/// replaced by the symbol it refers to.
pub(super) fn raise(ty: &core::Type, slots: &[Symbol]) -> Result<Ty, Invalid> {
    let name = |slot: &Slot| {
        let symbol = slots.get(slot.0).ok_or(Invalid::Slot)?;
        Ok(Reference::Definition(symbol.clone()))
    };
    let all = |types: &[core::Type]| -> Result<Vec<Ty>, Invalid> {
        types.iter().map(|ty| raise(ty, slots)).collect()
    };
    let one = |ty: &core::Type| raise(ty, slots).map(Box::new);
    Ok(match ty {
        core::Type::Never => Ty::Never,
        core::Type::Unknown => Ty::Unknown,
        core::Type::Null => Ty::Null,
        core::Type::Bool => Ty::Bool,
        core::Type::Int => Ty::Int,
        core::Type::Float => Ty::Float,
        core::Type::String => Ty::String,
        core::Type::Tag => Ty::Tag,
        core::Type::Literal(literal) => Ty::Literal(literal.clone()),
        core::Type::Record(fields) => {
            let mut raised = Vec::new();
            for (key, ty) in fields {
                let key = match key {
                    core::Key::Field(name) => Key::Field(name.clone()),
                    core::Key::Ability(slot) => Key::Ability(name(slot)?),
                };
                raised.push((key, raise(ty, slots)?));
            }
            Ty::Record(raised)
        }
        core::Type::Tuple(members) => Ty::Tuple(all(members)?),
        core::Type::Union(members) => Ty::Union(all(members)?),
        core::Type::List(member) => Ty::List(one(member)?),
        core::Type::Cell(member) => Ty::Cell(one(member)?),
        core::Type::Function { params, ret, row } => Ty::Function {
            params: all(params)?,
            ret: one(ret)?,
            row: row.iter().map(name).collect::<Result<_, _>>()?,
        },
        core::Type::Parameter(position) => Ty::Named(Reference::Parameter(*position), Vec::new()),
        core::Type::Reference(slot, args) => Ty::Named(name(slot)?, all(args)?),
    })
}

impl Verifier<'_> {
    /// What a type of the definition being verified means, in normal
    /// form. The type must be well-formed, and written exactly as
    /// checking writes it: a type has one form, so that it has one hash.
    pub(super) fn sound(&self, ty: &core::Type) -> Result<Ty, Invalid> {
        let raised = raise(ty, self.slots)?;
        self.well_formed(&raised)?;
        let normal = self.env.normalize(&raised);
        let mut known = true;
        let written = self.env.lower(&normal, &mut |symbol| {
            let position = self.slots.iter().position(|slot| slot == symbol);
            known &= position.is_some();
            Slot(position.unwrap_or(usize::MAX))
        });
        if known && written == *ty {
            Ok(normal)
        } else {
            Err(Invalid::Type)
        }
    }

    /// [`Self::sound`], of a type a value is tested against as the
    /// program runs.
    pub(super) fn tested(&self, ty: &core::Type) -> Result<Ty, Invalid> {
        let ty = self.sound(ty)?;
        if self.env.testable(&ty) {
            Ok(ty)
        } else {
            Err(Invalid::Shape(
                "a value is tested for a function type".into(),
            ))
        }
    }

    pub(super) fn sound_all(&self, types: &[core::Type]) -> Result<Vec<Ty>, Invalid> {
        types.iter().map(|ty| self.sound(ty)).collect()
    }

    /// Fails unless every name in the type is the kind of definition its
    /// place calls for, applied to as many arguments as it takes.
    fn well_formed(&self, ty: &Ty) -> Result<(), Invalid> {
        let ability = |name: &Reference| match name {
            Reference::Definition(symbol) if self.env.abilities.contains_key(symbol) => Ok(()),
            _ => Err(Invalid::Slot),
        };
        let all = |types: &[Ty]| types.iter().try_for_each(|ty| self.well_formed(ty));
        match ty {
            Ty::Never
            | Ty::Unknown
            | Ty::Null
            | Ty::Bool
            | Ty::Int
            | Ty::Float
            | Ty::String
            | Ty::Tag
            | Ty::Literal(_) => Ok(()),
            Ty::Record(fields) => fields.iter().try_for_each(|(key, ty)| {
                if let Key::Ability(name) = key {
                    ability(name)?;
                }
                self.well_formed(ty)
            }),
            Ty::Tuple(members) | Ty::Union(members) => all(members),
            Ty::List(member) | Ty::Cell(member) => self.well_formed(member),
            Ty::Function { params, ret, row } => {
                row.iter().try_for_each(ability)?;
                all(params)?;
                self.well_formed(ret)
            }
            Ty::Named(Reference::Parameter(position), args) => {
                if *position < self.env.bounds.len() && args.is_empty() {
                    Ok(())
                } else {
                    Err(Invalid::Type)
                }
            }
            Ty::Named(Reference::Definition(symbol), args) => {
                let info = self.env.types.get(symbol).ok_or(Invalid::Slot)?;
                if info.params != args.len() {
                    return Err(Invalid::Arity);
                }
                all(args)
            }
            Ty::Named(Reference::Local(_), _) => Err(Invalid::Type),
        }
    }
}
