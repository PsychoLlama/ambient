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
                self.intersect(self.normal(ty, absorb), brands.collect(), absorb)
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
            Ty::Tuple(members) => Ty::Tuple(members.each(|m| self.normal(m, absorb))),
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
                    params: params.each(|p| self.normal(p, absorb)),
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

impl Env {
    /// A normalized type intersected with others: records are made one
    /// record, and whatever is left is put on it as brands are. What
    /// is left that is no brand is no type, which
    /// `Checker::well_formed` says.
    fn intersect(&self, ty: Ty, with: Vec<Ty>, absorb: bool) -> Ty {
        let mut left = Vec::new();
        let ty = with.into_iter().fold(ty, |ty, other| {
            self.meet(&ty, &other, absorb).unwrap_or_else(|| {
                left.push(other);
                ty
            })
        });
        if left.is_empty() {
            ty
        } else {
            branded(ty, left)
        }
    }

    /// The record that is both of two records: every field of either,
    /// and a field of both at the type that is both of its types.
    /// Nothing if either is not a record, or a field has no such type.
    fn meet(&self, left: &Ty, right: &Ty, absorb: bool) -> Option<Ty> {
        let (Ty::Record(left), Ty::Record(right)) = (left, right) else {
            return None;
        };
        let mut fields = left.clone();
        for (key, ty) in right {
            match fields.iter_mut().find(|(held, _)| held == key) {
                Some((_, held)) => *held = self.both(held, ty, absorb)?,
                None => fields.push((key.clone(), ty.clone())),
            }
        }
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        Some(Ty::Record(fields))
    }

    /// The type that is both of two types, where one is written without
    /// a type of its own for it: the same type, two records, or one
    /// that is already the other.
    fn both(&self, left: &Ty, right: &Ty, absorb: bool) -> Option<Ty> {
        if left == right {
            return Some(left.clone());
        }
        if let Some(record) = self.meet(left, right, absorb) {
            return Some(record);
        }
        // Asking unfolds, which a definition being unfolded cannot.
        if absorb && self.subtype(left, right) {
            return Some(left.clone());
        }
        (absorb && self.subtype(right, left)).then(|| right.clone())
    }
}
