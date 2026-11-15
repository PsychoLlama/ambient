//! The one form of a type: transparent names expanded, record fields
//! ordered, unions simplified. Every other operation expects it.

use super::{Env, Ty, branded, substitute};
use crate::literal::Literal;
use crate::surface::Reference;

impl Env {
    /// The type in the form every other operation expects: transparent
    /// names expanded, record fields ordered, unions simplified.
    #[must_use]
    pub fn normalize(&self, ty: &Ty) -> Ty {
        self.normal(ty, true)
    }

    /// [`Self::normalize`], with unions simplified only if `absorb`.
    pub(super) fn normal(&self, ty: &Ty, absorb: bool) -> Ty {
        match ty {
            Ty::Never
            | Ty::Unknown
            | Ty::Null
            | Ty::Bool
            | Ty::Int
            | Ty::Float
            | Ty::String
            | Ty::Bytes
            | Ty::Tag
            | Ty::Literal(_)
            | Ty::Brand(_)
            | Ty::Bound(_)
            | Ty::Named(Reference::Local(_) | Reference::Parameter(_), _) => ty.clone(),
            Ty::Branded(ty, brands) => {
                let brands = brands.iter().map(|brand| self.normal(brand, absorb));
                branded(self.normal(ty, absorb), brands.collect())
            }
            Ty::Generic { bounds, function } => Ty::Generic {
                bounds: bounds.iter().map(|b| self.normal(b, absorb)).collect(),
                function: Box::new(self.normal(function, absorb)),
            },
            Ty::Record(fields) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .map(|(key, ty)| (key.clone(), self.normal(ty, absorb)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Ty::Record(fields)
            }
            Ty::Tuple(members) => {
                Ty::Tuple(members.iter().map(|m| self.normal(m, absorb)).collect())
            }
            Ty::List(member) => Ty::List(Box::new(self.normal(member, absorb))),
            Ty::Cell(member) => Ty::Cell(Box::new(self.normal(member, absorb))),
            Ty::Iterator(member) => Ty::Iterator(Box::new(self.normal(member, absorb))),
            Ty::Method { needs, function } => Ty::Method {
                needs: Box::new(self.normal(needs, absorb)),
                function: Box::new(self.normal(function, absorb)),
            },
            Ty::Function { params, ret, row } => {
                let mut row = row.clone();
                row.sort();
                row.dedup();
                Ty::Function {
                    params: params.iter().map(|p| self.normal(p, absorb)).collect(),
                    ret: Box::new(self.normal(ret, absorb)),
                    row,
                }
            }
            Ty::Union(members) => {
                let members = members.iter().map(|m| self.normal(m, absorb)).collect();
                self.join(members, absorb)
            }
            Ty::Named(Reference::Definition(symbol), args) => {
                let args: Vec<Ty> = args.iter().map(|arg| self.normal(arg, absorb)).collect();
                match self.types.get(symbol) {
                    Some(info) if !info.recursive => {
                        self.normal(&substitute(&info.ty, &args), absorb)
                    }
                    _ => Ty::Named(Reference::Definition(symbol.clone()), args),
                }
            }
        }
    }

    /// A normalized type with any recursive name at its head replaced by
    /// its definition, so its outermost constructor can be inspected.
    ///
    /// The unions of the definition are not simplified: simplifying asks
    /// whether one member is a subtype of another, which unfolds, and a
    /// definition that holds a union with itself in it would never be done.
    #[must_use]
    pub fn unfold(&self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        // A contractive type reaches a constructor within as many steps as
        // there are definitions.
        for _ in 0..=self.types.len() {
            let Ty::Named(Reference::Definition(symbol), args) = &ty else {
                break;
            };
            let Some(info) = self.recursive(symbol) else {
                break;
            };
            ty = self.normal(&substitute(&info.ty, args), false);
        }
        ty
    }

    /// The union of normalized types, simplified: nested unions flattened,
    /// members that another member already covers dropped.
    #[must_use]
    pub fn union(&self, members: Vec<Ty>) -> Ty {
        self.join(members, true)
    }

    /// [`Self::union`], dropping covered members only if `absorb`.
    pub(super) fn join(&self, members: Vec<Ty>, absorb: bool) -> Ty {
        let mut flat = Vec::new();
        for member in members {
            match member {
                Ty::Union(inner) => flat.extend(inner),
                Ty::Never => {}
                other => flat.push(other),
            }
        }
        let mut kept: Vec<Ty> = Vec::new();
        for member in flat {
            if !absorb {
                kept.push(member);
                continue;
            }
            if kept.iter().any(|other| self.subtype(&member, other)) {
                continue;
            }
            kept.retain(|other| !self.subtype(other, &member));
            kept.push(member);
        }
        let boolean = |value| Ty::Literal(Literal::Bool(value));
        if kept.contains(&boolean(true)) && kept.contains(&boolean(false)) {
            kept.retain(|member| !matches!(member, Ty::Literal(Literal::Bool(_))));
            kept.push(Ty::Bool);
        }
        kept.sort();
        kept.dedup();
        match kept.len() {
            0 => Ty::Never,
            1 => kept.remove(0),
            _ => Ty::Union(kept),
        }
    }
}
