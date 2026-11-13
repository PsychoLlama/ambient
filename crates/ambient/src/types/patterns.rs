//! What a pattern does to a type: the values it matches, the values it
//! lets through, and what its binders hold.

use super::{Env, Ty, TyKey, entry, field};
use crate::literal::Literal;
use crate::surface::{Key, Pattern, Reference, Shape};

type Pat = Pattern<Reference>;

impl Env {
    /// The values of type `ty` that `pattern` matches, as a type; `None` if
    /// there are none.
    #[must_use]
    pub fn narrow(&self, ty: &Ty, pattern: &Pat) -> Option<Ty> {
        let ty = self.unfold(ty);
        if let Ty::Union(members) = &ty {
            let parts: Vec<Ty> = members
                .iter()
                .filter_map(|member| self.narrow(member, pattern))
                .collect();
            return (!parts.is_empty()).then(|| self.union(parts));
        }
        if ty == Ty::Never {
            return None;
        }
        match pattern {
            Pattern::Wildcard => Some(ty),
            Pattern::Bind(_, inner) => self.narrow(&ty, inner),
            Pattern::Literal(literal) => {
                let literal = Ty::Literal(literal.clone());
                self.subtype(&literal, &ty).then_some(literal)
            }
            Pattern::Is(tested) => {
                let tested = self.normalize(tested);
                if self.subtype(&ty, &tested) {
                    Some(ty)
                } else if self.disjoint(&ty, &tested) {
                    None
                } else {
                    // The value is described by both types. Either is a
                    // sound answer; the tested type is the one asked for.
                    Some(tested)
                }
            }
            Pattern::Record(patterns) => {
                let mut fields = match ty {
                    Ty::Record(fields) => fields,
                    Ty::Unknown => Vec::new(),
                    _ => return None,
                };
                for (name, pattern) in patterns {
                    // A field the type does not mention may still be there.
                    let have = field(&fields, name).cloned().unwrap_or(Ty::Unknown);
                    let narrowed = self.narrow(&have, pattern)?;
                    fields.retain(|(key, _)| !key.is_field(name));
                    fields.push((Key::Field(name.clone()), narrowed));
                }
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Some(Ty::Record(fields))
            }
            Pattern::Tuple(patterns) => {
                let members: Vec<Ty> = match ty {
                    Ty::Tuple(members) if members.len() == patterns.len() => members,
                    // A list type describes tuples of every length.
                    Ty::List(member) => vec![*member; patterns.len()],
                    Ty::Unknown => vec![Ty::Unknown; patterns.len()],
                    _ => return None,
                };
                let narrowed: Option<Vec<Ty>> = members
                    .iter()
                    .zip(patterns)
                    .map(|(member, pattern)| self.narrow(member, pattern))
                    .collect();
                narrowed.map(Ty::Tuple)
            }
        }
    }

    /// Whether `pattern` matches every value of type `ty`.
    #[must_use]
    pub fn covers(&self, ty: &Ty, pattern: &Pat) -> bool {
        let ty = self.unfold(ty);
        if let Ty::Union(members) = &ty {
            return members.iter().all(|member| self.covers(member, pattern));
        }
        match pattern {
            Pattern::Wildcard => true,
            Pattern::Bind(_, inner) => self.covers(&ty, inner),
            Pattern::Literal(literal) => ty == Ty::Literal(literal.clone()),
            Pattern::Is(tested) => self.subtype(&ty, &self.normalize(tested)),
            Pattern::Record(patterns) => match &ty {
                Ty::Record(fields) => patterns.iter().all(|(name, pattern)| {
                    field(fields, name).is_some_and(|ty| self.covers(ty, pattern))
                }),
                _ => false,
            },
            Pattern::Tuple(patterns) => match &ty {
                Ty::Tuple(members) => {
                    members.len() == patterns.len()
                        && members.iter().zip(patterns).all(|(m, p)| self.covers(m, p))
                }
                _ => false,
            },
        }
    }

    /// The values of type `ty` that `pattern` may fail to match, as a type.
    /// It may describe more values than that, never fewer.
    #[must_use]
    pub fn remainder(&self, ty: &Ty, pattern: &Pat) -> Ty {
        let ty = self.unfold(ty);
        match &ty {
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.remainder(m, pattern)).collect();
                return self.union(parts);
            }
            Ty::Bool => {
                let parts = [false, true]
                    .map(|value| self.remainder(&Ty::Literal(Literal::Bool(value)), pattern));
                return self.union(parts.to_vec());
            }
            _ => {}
        }
        if self.covers(&ty, pattern) {
            return Ty::Never;
        }
        // When exactly one member of a record or tuple pattern can fail,
        // what is left is the same type with that member narrowed.
        match (pattern, &ty) {
            (Pattern::Bind(_, inner), _) => self.remainder(&ty, inner),
            (Pattern::Record(patterns), Ty::Record(fields)) => {
                let all_present = patterns
                    .iter()
                    .all(|(name, _)| field(fields, name).is_some());
                let mut open = patterns.iter().filter(|(name, pattern)| {
                    !field(fields, name).is_some_and(|ty| self.covers(ty, pattern))
                });
                let (Some((name, pattern)), None, true) = (open.next(), open.next(), all_present)
                else {
                    return ty;
                };
                let fields = fields
                    .iter()
                    .map(|(field, ty)| {
                        let ty = if field.is_field(name) {
                            self.remainder(ty, pattern)
                        } else {
                            ty.clone()
                        };
                        (field.clone(), ty)
                    })
                    .collect();
                uninhabited_if_member_is(Ty::Record(fields))
            }
            (Pattern::Tuple(patterns), Ty::Tuple(members)) if patterns.len() == members.len() => {
                let mut open = (0..members.len())
                    .filter(|index| !self.covers(&members[*index], &patterns[*index]));
                let (Some(index), None) = (open.next(), open.next()) else {
                    return ty;
                };
                let mut members = members.clone();
                members[index] = self.remainder(&members[index], &patterns[index]);
                uninhabited_if_member_is(Ty::Tuple(members))
            }
            _ => ty,
        }
    }

    /// The type under `key` of a value of type `ty`, if every such value
    /// has one.
    #[must_use]
    pub fn field(&self, ty: &Ty, key: &TyKey) -> Option<Ty> {
        match self.unfold(ty) {
            Ty::Never => Some(Ty::Never),
            Ty::Named(Reference::Parameter(position), _) => {
                self.field(self.bounds.get(position)?, key)
            }
            Ty::Record(fields) => entry(&fields, key).map(Ty::read),
            Ty::Union(members) => {
                let fields: Option<Vec<Ty>> = members.iter().map(|m| self.field(m, key)).collect();
                fields.map(|fields| self.union(fields))
            }
            _ => None,
        }
    }

    /// What reading the method under `key` of a value of type `ty`
    /// gives, if every such value is a record whose type lists a method
    /// there.
    #[must_use]
    pub fn method(&self, ty: &Ty, key: &TyKey) -> Option<Ty> {
        match self.unfold(ty) {
            Ty::Named(Reference::Parameter(position), _) => {
                self.method(self.bounds.get(position)?, key)
            }
            Ty::Record(fields) => match entry(&fields, key)? {
                Ty::Method { function, .. } => Some(self.unfold(function)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether a value of type `held` may be held under `key`: always
    /// under a name or an ability, and under a key if reading it gives
    /// the key's type.
    #[must_use]
    pub fn holds(&self, key: &TyKey, held: &Ty) -> bool {
        let Key::Ability(Reference::Definition(named)) = key else {
            return true;
        };
        let Some(info) = self.keys.get(named) else {
            return true;
        };
        self.subtype(&self.normalize(&held.read()), &info.ty)
    }

    /// The built-in shape every value of type `ty` has, if they share
    /// one that has methods: a record has fields, and a type parameter
    /// or `unknown` says nothing of shape.
    #[must_use]
    pub fn shape(&self, ty: &Ty) -> Option<Shape> {
        Some(match self.unfold(ty) {
            Ty::Null => Shape::Null,
            Ty::Bool | Ty::Literal(Literal::Bool(_)) => Shape::Bool,
            Ty::Int | Ty::Literal(Literal::Int(_)) => Shape::Int,
            Ty::Float | Ty::Literal(Literal::Float(_)) => Shape::Float,
            Ty::String | Ty::Literal(Literal::String(_)) => Shape::String,
            Ty::Tag | Ty::Literal(Literal::Tag { .. }) => Shape::Tag,
            Ty::Tuple(_) => Shape::Tuple,
            Ty::List(_) => Shape::List,
            Ty::Function { .. } => Shape::Function,
            Ty::Cell(_) => Shape::Cell,
            Ty::Union(members) => {
                let mut shapes = members.iter().map(|member| self.shape(member));
                let first = shapes.next()??;
                return shapes.all(|shape| shape == Some(first)).then_some(first);
            }
            _ => return None,
        })
    }

    /// The type of member `index` of a tuple-shaped value of type `ty`.
    #[must_use]
    pub fn member(&self, ty: &Ty, index: usize) -> Ty {
        match ty {
            Ty::Tuple(members) => members.get(index).cloned().unwrap_or(Ty::Unknown),
            Ty::List(member) => (**member).clone(),
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.member(m, index)).collect();
                self.union(parts)
            }
            _ => Ty::Unknown,
        }
    }
}

/// A record or tuple with a member no value can have has no values itself.
fn uninhabited_if_member_is(ty: Ty) -> Ty {
    let empty = match &ty {
        Ty::Record(fields) => fields.iter().any(|(_, ty)| *ty == Ty::Never),
        Ty::Tuple(members) => members.contains(&Ty::Never),
        _ => false,
    };
    if empty { Ty::Never } else { ty }
}
