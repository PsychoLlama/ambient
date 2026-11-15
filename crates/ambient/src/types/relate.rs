//! The relations between two types: whether one is within the other, and
//! whether they share no value.

use super::{Env, Ty, Variance, booleans, entry, instantiate, primitive_of};
use crate::surface::Reference;

/// The longest chain of assumptions a subtyping proof may build. Regular
/// types never come near it; it is what makes an irregular one fail rather
/// than run forever.
const DEPTH: usize = 256;

/// The same, where a type being unfolded has no end of unfoldings and
/// each may be larger than the last: it is then being compared with
/// something other than itself, which a few unfoldings settle or none
/// will.
const SHALLOW: usize = 16;

/// A subtyping proof under way.
#[derive(Default)]
struct Proof {
    /// The judgements this one is being proved under.
    assumed: Vec<(Ty, Ty)>,
    /// The bound of each parameter of the generic function types the
    /// proof is inside, outermost first. Inside one its parameters are
    /// type parameters like a definition's, numbered after those.
    opened: Vec<Ty>,
}

impl Env {
    /// Whether one generic function type is within another: they bind
    /// as many parameters, with the same bounds, and with the same
    /// types standing for those the one function type is within the
    /// other. Bounds must be the same, not one within the other: with
    /// that, subtyping is no longer decidable.
    fn generic(&self, sub: (&[Ty], &Ty), sup: (&[Ty], &Ty), proof: &mut Proof) -> bool {
        let ((bounds, function), (want_bounds, want)) = (sub, sup);
        if bounds.len() != want_bounds.len() {
            return false;
        }
        let base = self.parameters() + proof.opened.len();
        let fresh: Vec<Ty> = (base..base + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        let open = |ty: &Ty| self.normalize(&instantiate(ty, &fresh));
        let (opened, assumed) = (proof.opened.len(), proof.assumed.len());
        proof.opened.extend(bounds.iter().map(&open));
        let same = bounds.iter().zip(want_bounds).all(|(have, want)| {
            let (have, want) = (open(have), open(want));
            self.sub(&have, &want, proof) && self.sub(&want, &have, proof)
        });
        let holds = same && self.sub(&open(function), &open(want), proof);
        // Whatever was assumed of these parameters says nothing of the
        // next ones to take their numbers.
        proof.opened.truncate(opened);
        proof.assumed.truncate(assumed);
        holds
    }

    /// Whether one use of a recursive definition is within another use
    /// of the same one, by how the definition varies with each of its
    /// arguments; or nothing, if they are not that.
    fn by_arguments(&self, sub: &Ty, sup: &Ty, proof: &mut Proof) -> Option<bool> {
        let (
            Ty::Named(Reference::Definition(name), have),
            Ty::Named(Reference::Definition(other), want),
        ) = (sub, sup)
        else {
            return None;
        };
        let info = self.types.get(name).filter(|_| name == other)?;
        if info.variance.len() != have.len() || have.len() != want.len() {
            return None;
        }
        let each = info.variance.iter().zip(have.iter().zip(want));
        Some(each.into_iter().all(|(variance, (have, want))| {
            let covariant = matches!(variance, Variance::Covariant | Variance::Invariant);
            let contravariant = matches!(variance, Variance::Contravariant | Variance::Invariant);
            (!covariant || self.sub(have, want, proof))
                && (!contravariant || self.sub(want, have, proof))
        }))
    }

    /// Whether unfolding `ty`, if it names a definition, is known to end.
    fn unfolds(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(Reference::Definition(symbol), _) => {
                self.types.get(symbol).is_none_or(|info| info.finite)
            }
            _ => true,
        }
    }

    /// Whether every value `sub` describes is one `sup` describes. Both are
    /// normalized.
    #[must_use]
    pub fn subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        self.sub(sub, sup, &mut Proof::default())
    }

    /// `proof` holds the judgements this one is being proved under.
    /// Meeting one again proves it: subtyping is coinductive.
    fn sub(&self, sub: &Ty, sup: &Ty, proof: &mut Proof) -> bool {
        if sub == sup {
            return true;
        }
        match (sub, sup) {
            (Ty::Never, _) | (_, Ty::Unknown) => true,
            (Ty::Named(Reference::Definition(_), _), _)
            | (_, Ty::Named(Reference::Definition(_), _)) => {
                // Two uses of one definition are compared by their
                // arguments first, and only so if unfolding it may not end.
                match self.by_arguments(sub, sup, proof) {
                    Some(true) => return true,
                    Some(false) if !self.unfolds(sub) => return false,
                    _ => {}
                }
                let judgement = (sub.clone(), sup.clone());
                if proof.assumed.contains(&judgement) {
                    return true;
                }
                let ends = self.unfolds(sub) && self.unfolds(sup);
                if proof.assumed.len() >= if ends { DEPTH } else { SHALLOW } {
                    return false;
                }
                let (below, above) = (self.unfold(sub), self.unfold(sup));
                if below == *sub && above == *sup {
                    return false;
                }
                let depth = proof.assumed.len();
                proof.assumed.push(judgement);
                let holds = self.sub(&below, &above, proof);
                if !holds {
                    // A judgement that failed must not stay assumed.
                    proof.assumed.truncate(depth);
                }
                holds
            }
            (Ty::Union(members), _) => members.iter().all(|m| self.sub(m, sup, proof)),
            // A type parameter is known only by its bound.
            (Ty::Named(Reference::Parameter(position), _), _) => {
                let member = matches!(sup, Ty::Union(members) if members.contains(sub));
                let opened = || proof.opened.get(position - self.parameters()).cloned();
                let bound = self.bound(*position).or_else(opened);
                member || bound.is_some_and(|bound| self.sub(&bound, sup, proof))
            }
            (Ty::Bool, Ty::Union(_)) => self.sub(&booleans(), sup, proof),
            (_, Ty::Union(members)) => members.iter().any(|m| self.sub(sub, m, proof)),
            // A branded value is one of the type under its brands, and
            // one with fewer brands.
            (Ty::Branded(have, brands), Ty::Branded(want, wanted)) => {
                wanted.iter().all(|brand| brands.contains(brand)) && self.sub(have, want, proof)
            }
            (Ty::Branded(have, _), _) => self.sub(have, sup, proof),
            (Ty::Literal(literal), _) => primitive_of(literal) == *sup,
            (Ty::Record(have), Ty::Record(want)) => want.iter().all(|(key, want)| {
                entry(have, key).is_some_and(|have| self.sub(have, want, proof))
            }),
            (Ty::Tuple(have), Ty::Tuple(want)) => {
                have.len() == want.len()
                    && have.iter().zip(want).all(|(h, w)| self.sub(h, w, proof))
            }
            // Neither is ever written to, so both are covariant.
            (Ty::List(have), Ty::List(want)) | (Ty::Iterator(have), Ty::Iterator(want)) => {
                self.sub(have, want, proof)
            }
            // A method that needs less of its receiver stands in for one
            // that needs more.
            (
                Ty::Method { needs, function },
                Ty::Method {
                    needs: want_needs,
                    function: want,
                },
            ) => self.sub(want_needs, needs, proof) && self.sub(function, want, proof),
            // Read, a method is the function it gives; what it needed
            // is then forgotten.
            (Ty::Method { function, .. }, _) => self.sub(function, sup, proof),
            // A cell is written as well as read, so it is the same type
            // or it is not a subtype.
            (Ty::Cell(have), Ty::Cell(want)) => {
                self.sub(have, want, proof) && self.sub(want, have, proof)
            }
            (
                Ty::Generic { bounds, function },
                Ty::Generic {
                    bounds: want_bounds,
                    function: want,
                },
            ) => self.generic((bounds, function), (want_bounds, want), proof),
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
                        .all(|(want, have)| self.sub(want, have, proof))
                    && self.sub(ret, want_ret, proof)
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
            // A brand takes no value from the type it is on.
            (Ty::Branded(ty, _), other) | (other, Ty::Branded(ty, _)) => {
                self.apart(ty, other, next)
            }
            // Two list types share the empty list, two iterator types an
            // iterator of nothing, and two function types may share a
            // function.
            (Ty::Named(Reference::Parameter(position), _), other)
            | (other, Ty::Named(Reference::Parameter(position), _)) => self
                .bound(*position)
                .is_some_and(|bound| self.apart(&bound, other, next)),
            (Ty::Unknown | Ty::Named(..), _)
            | (_, Ty::Unknown | Ty::Named(..))
            | (Ty::List(_), Ty::List(_))
            | (Ty::Iterator(_), Ty::Iterator(_))
            | (Ty::Generic { .. }, Ty::Generic { .. })
            | (Ty::Function { .. }, Ty::Function { .. }) => false,
            (Ty::Union(members), other) | (other, Ty::Union(members)) => {
                members.iter().all(|m| self.apart(m, other, next))
            }
            // Read, a method is a function, and two functions may be one.
            (Ty::Method { .. }, _) | (_, Ty::Method { .. }) => false,
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
