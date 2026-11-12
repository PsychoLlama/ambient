//! Match: what each arm matches, what it lets through, and whether
//! anything is left at the end.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Arm, Expr, Key, Pattern, Reference};
use crate::types::{Ty, field};

type Pat = Pattern<Reference>;

impl Checker<'_> {
    pub(super) fn matching(
        &mut self,
        subject: &Expr<Reference>,
        arms: &[Arm<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (subject, mut remaining) = self.infer(subject)?;
        for arm in arms {
            let mut result = Ok(());
            pattern_types(&arm.pattern, &mut |ty| {
                if result.is_ok() {
                    result = self.well_formed(ty);
                }
            });
            result?;
        }
        let mut core_arms = Vec::new();
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            // An arm sees only what earlier arms let through.
            let Some(matched) = self.narrow(&remaining, &arm.pattern) else {
                return Err(Error::UnreachableArm {
                    symbol: self.symbol.clone(),
                    arm: index,
                });
            };
            let pattern = self.bind(&matched, &arm.pattern);
            let (body, ty) = self.infer(&arm.body)?;
            core_arms.push((pattern, body));
            results.push(ty);
            remaining = self.remainder(&remaining, &arm.pattern);
        }
        if remaining != Ty::Never {
            return Err(Error::NotExhaustive {
                symbol: self.symbol.clone(),
                remaining: self.env.show(&remaining),
            });
        }
        let core = core::Expr::Match {
            subject: Box::new(subject),
            arms: core_arms,
        };
        Ok((core, self.env.union(results)))
    }

    /// The values of type `ty` that `pattern` matches, as a type; `None` if
    /// there are none.
    pub(super) fn narrow(&self, ty: &Ty, pattern: &Pat) -> Option<Ty> {
        let ty = self.env.unfold(ty);
        if let Ty::Union(members) = &ty {
            let parts: Vec<Ty> = members
                .iter()
                .filter_map(|member| self.narrow(member, pattern))
                .collect();
            return (!parts.is_empty()).then(|| self.env.union(parts));
        }
        if ty == Ty::Never {
            return None;
        }
        match pattern {
            Pattern::Wildcard => Some(ty),
            Pattern::Bind(_, inner) => self.narrow(&ty, inner),
            Pattern::Literal(literal) => {
                let literal = Ty::Literal(literal.clone());
                self.env.subtype(&literal, &ty).then_some(literal)
            }
            Pattern::Is(tested) => {
                let tested = self.env.normalize(tested);
                if self.env.subtype(&ty, &tested) {
                    Some(ty)
                } else if self.env.disjoint(&ty, &tested) {
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
    pub(super) fn covers(&self, ty: &Ty, pattern: &Pat) -> bool {
        let ty = self.env.unfold(ty);
        if let Ty::Union(members) = &ty {
            return members.iter().all(|member| self.covers(member, pattern));
        }
        match pattern {
            Pattern::Wildcard => true,
            Pattern::Bind(_, inner) => self.covers(&ty, inner),
            Pattern::Literal(literal) => ty == Ty::Literal(literal.clone()),
            Pattern::Is(tested) => self.env.subtype(&ty, &self.env.normalize(tested)),
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
    pub(super) fn remainder(&self, ty: &Ty, pattern: &Pat) -> Ty {
        let ty = self.env.unfold(ty);
        match &ty {
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.remainder(m, pattern)).collect();
                return self.env.union(parts);
            }
            Ty::Bool => {
                let parts = [false, true]
                    .map(|value| self.remainder(&Ty::Literal(Literal::Bool(value)), pattern));
                return self.env.union(parts.to_vec());
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

    /// Gives every binder in `pattern` its type, in binding order, and
    /// returns the core pattern. `ty` is what the pattern matched.
    pub(super) fn bind(&mut self, ty: &Ty, pattern: &Pat) -> core::Pattern {
        let ty = self.env.unfold(ty);
        match pattern {
            Pattern::Wildcard => core::Pattern::Wildcard,
            Pattern::Literal(literal) => core::Pattern::Literal(literal.clone()),
            Pattern::Is(tested) => {
                let tested = self.env.normalize(tested);
                core::Pattern::Is(self.lower(&tested))
            }
            Pattern::Bind(_, inner) => {
                self.bind_local(ty.clone());
                core::Pattern::Bind(Box::new(self.bind(&ty, inner)))
            }
            Pattern::Record(patterns) => core::Pattern::Record(
                patterns
                    .iter()
                    .map(|(name, pattern)| {
                        let key = Key::Field(name.clone());
                        let member = self.field(&ty, &key).unwrap_or(Ty::Unknown);
                        (name.clone(), self.bind(&member, pattern))
                    })
                    .collect(),
            ),
            Pattern::Tuple(patterns) => core::Pattern::Tuple(
                patterns
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        let member = self.member(&ty, index);
                        self.bind(&member, pattern)
                    })
                    .collect(),
            ),
        }
    }

    /// The type of member `index` of a tuple-shaped value of type `ty`.
    pub(super) fn member(&self, ty: &Ty, index: usize) -> Ty {
        match ty {
            Ty::Tuple(members) => members.get(index).cloned().unwrap_or(Ty::Unknown),
            Ty::List(member) => (**member).clone(),
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.member(m, index)).collect();
                self.env.union(parts)
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

/// Calls `visit` with every type a pattern tests against.
fn pattern_types(pattern: &Pat, visit: &mut impl FnMut(&Ty)) {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Record(fields) => fields.iter().for_each(|(_, p)| pattern_types(p, visit)),
        Pattern::Tuple(members) => members.iter().for_each(|p| pattern_types(p, visit)),
        Pattern::Is(ty) => visit(ty),
        Pattern::Bind(_, inner) => pattern_types(inner, visit),
    }
}
