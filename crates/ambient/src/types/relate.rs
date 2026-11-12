//! The relations between two types: whether one is within the other, and
//! whether they share no value.

use super::{Env, Ty, booleans, entry, primitive_of};
use crate::surface::Reference;

/// The longest chain of assumptions a subtyping proof may build. Regular
/// types never come near it; it is what makes an irregular one fail rather
/// than run forever.
const DEPTH: usize = 256;

impl Env {
    /// Whether every value `sub` describes is one `sup` describes. Both are
    /// normalized.
    #[must_use]
    pub fn subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        self.sub(sub, sup, &mut Vec::new())
    }

    /// `assumed` holds the judgements this one is being proved under.
    /// Meeting one again proves it: subtyping is coinductive.
    fn sub(&self, sub: &Ty, sup: &Ty, assumed: &mut Vec<(Ty, Ty)>) -> bool {
        if sub == sup {
            return true;
        }
        match (sub, sup) {
            (Ty::Never, _) | (_, Ty::Unknown) => true,
            (Ty::Named(Reference::Definition(_), _), _)
            | (_, Ty::Named(Reference::Definition(_), _)) => {
                let judgement = (sub.clone(), sup.clone());
                if assumed.contains(&judgement) {
                    return true;
                }
                if assumed.len() >= DEPTH {
                    return false;
                }
                let (below, above) = (self.unfold(sub), self.unfold(sup));
                if below == *sub && above == *sup {
                    return false;
                }
                let depth = assumed.len();
                assumed.push(judgement);
                let holds = self.sub(&below, &above, assumed);
                if !holds {
                    // A judgement that failed must not stay assumed.
                    assumed.truncate(depth);
                }
                holds
            }
            (Ty::Union(members), _) => members.iter().all(|m| self.sub(m, sup, assumed)),
            // A type parameter is known only by its bound.
            (Ty::Named(Reference::Parameter(position), _), _) => {
                let member = matches!(sup, Ty::Union(members) if members.contains(sub));
                member
                    || self
                        .bounds
                        .get(*position)
                        .is_some_and(|bound| self.sub(bound, sup, assumed))
            }
            (Ty::Bool, Ty::Union(_)) => self.sub(&booleans(), sup, assumed),
            (_, Ty::Union(members)) => members.iter().any(|m| self.sub(sub, m, assumed)),
            (Ty::Literal(literal), _) => primitive_of(literal) == *sup,
            (Ty::Record(have), Ty::Record(want)) => want.iter().all(|(key, want)| {
                entry(have, key).is_some_and(|have| self.sub(have, want, assumed))
            }),
            (Ty::Tuple(have), Ty::Tuple(want)) => {
                have.len() == want.len()
                    && have.iter().zip(want).all(|(h, w)| self.sub(h, w, assumed))
            }
            (Ty::List(have), Ty::List(want)) => self.sub(have, want, assumed),
            // A cell is written as well as read, so it is the same type
            // or it is not a subtype.
            (Ty::Cell(have), Ty::Cell(want)) => {
                self.sub(have, want, assumed) && self.sub(want, have, assumed)
            }
            (
                Ty::Function { params, ret, row },
                Ty::Function {
                    params: want_params,
                    ret: want_ret,
                    row: want_row,
                },
            ) => {
                // A function that takes less from its caller can stand in
                // for one that takes more.
                row.iter().all(|ability| want_row.contains(ability))
                    && params.len() == want_params.len()
                    && want_params
                        .iter()
                        .zip(params)
                        .all(|(want, have)| self.sub(want, have, assumed))
                    && self.sub(ret, want_ret, assumed)
            }
            _ => false,
        }
    }

    /// Whether no value is described by both types. Types overlap unless
    /// this proves otherwise.
    #[must_use]
    pub fn disjoint(&self, left: &Ty, right: &Ty) -> bool {
        self.apart(left, right, 0)
    }

    fn apart(&self, left: &Ty, right: &Ty, depth: usize) -> bool {
        // Past any recursive type's own depth nothing new can be learned;
        // answering "may overlap" is always safe.
        if depth > self.types.len() + 8 {
            return false;
        }
        let (left, right) = (self.unfold(left), self.unfold(right));
        let next = depth + 1;
        match (&left, &right) {
            (Ty::Never, _) | (_, Ty::Never) => true,
            // Two list types share the empty list; two function types may
            // share a function.
            (Ty::Named(Reference::Parameter(position), _), other)
            | (other, Ty::Named(Reference::Parameter(position), _)) => self
                .bounds
                .get(*position)
                .is_some_and(|bound| self.apart(bound, other, next)),
            (Ty::Unknown | Ty::Named(..), _)
            | (_, Ty::Unknown | Ty::Named(..))
            | (Ty::List(_), Ty::List(_))
            | (Ty::Function { .. }, Ty::Function { .. }) => false,
            (Ty::Union(members), other) | (other, Ty::Union(members)) => {
                members.iter().all(|m| self.apart(m, other, next))
            }
            (Ty::Literal(a), Ty::Literal(b)) => a != b,
            (Ty::Literal(literal), other) | (other, Ty::Literal(literal)) => {
                primitive_of(literal) != *other
            }
            (Ty::Record(a), Ty::Record(b)) => a
                .iter()
                .any(|(key, a)| entry(b, key).is_some_and(|b| self.apart(a, b, next))),
            (Ty::Tuple(a), Ty::Tuple(b)) => {
                a.len() != b.len() || a.iter().zip(b).any(|(a, b)| self.apart(a, b, next))
            }
            _ => left != right,
        }
    }
}
