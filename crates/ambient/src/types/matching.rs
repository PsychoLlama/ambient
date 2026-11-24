//! Type matches: deciding one, what one left open is within, and the
//! type matches a value match over an open subject gives its binders.
//! See "Type matches" in `docs/spec/types.md`.

use super::{Env, Ty, Variance, field, mentions_parameter};
use crate::surface::{Key, Pattern, Reference, TypeMatch};

type Pat = Pattern<Reference>;
type Matching = TypeMatch<Reference>;

impl Env {
    /// A normalized type match decided, or left as it is while what it
    /// gives depends on a type parameter.
    pub(super) fn decide(&self, matching: Matching) -> Ty {
        if self.undecided(&matching) {
            Ty::Match(Box::new(matching))
        } else {
            self.evaluate(&matching, &[])
        }
    }

    /// Whether `ty` is a type match that can be decided: one an
    /// unfolding made, whose arguments are now in.
    #[must_use]
    pub(super) fn decidable(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Match(matching) if !self.undecided(matching))
    }

    /// Whether each arm of a type match gives a wider type of wider
    /// binders: none holds one to more than being read.
    pub(super) fn widens(&self, matching: &Matching) -> bool {
        let base = self.parameters();
        matching.arms.iter().all(|(pattern, ty)| {
            let count = pattern.binders();
            let fresh: Vec<Ty> = (base..base + count)
                .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
                .collect();
            let opened = fill(ty, &fresh, 0);
            let held = self.variance(&opened, base + count);
            held[base..]
                .iter()
                .all(|variance| matches!(variance, Variance::Covariant | Variance::Unused))
        })
    }

    /// What a type match gives with each type parameter it depends on
    /// at its bound: what it gives, if it is decided, and otherwise a
    /// type it is within. A test of a type that is open may match
    /// anything, so it is asked as a wildcard, and taken to always
    /// match nothing.
    #[must_use]
    pub fn upper(&self, matching: &Matching) -> Ty {
        let arms = matching
            .arms
            .iter()
            .map(|(pattern, ty)| (loosened(pattern), ty.clone()));
        let loose = TypeMatch {
            subject: matching.subject.clone(),
            arms: arms.collect(),
        };
        let always = matching
            .arms
            .iter()
            .map(|(pattern, _)| !names_open(pattern));
        self.evaluate(&loose, &always.collect::<Vec<_>>())
    }

    /// What an arm of a type match over `subject` gives, with each of
    /// its binders as what its pattern narrows the subject to, and a
    /// test of an open type taken to match anything.
    pub(super) fn arm_known(&self, subject: &Ty, pattern: &Pat, ty: &Ty) -> Ty {
        let pattern = loosened(pattern);
        let mut bound = Vec::new();
        if let Some(matched) = self.narrow(subject, &pattern) {
            self.binders(&matched, &pattern, &mut bound);
        }
        fill(ty, &bound, 0)
    }

    /// Each member of the subject taken by every arm that may match it,
    /// up to the first that always does: the union of what they give.
    /// `last` says which arms may be that first; every arm may if it is
    /// empty.
    fn evaluate(&self, matching: &Matching, last: &[bool]) -> Ty {
        let members = match self.unfold(&matching.subject) {
            Ty::Union(members) => members,
            other => vec![other],
        };
        let mut given = Vec::new();
        for member in &members {
            for (index, (pattern, ty)) in matching.arms.iter().enumerate() {
                let Some(matched) = self.narrow(member, pattern) else {
                    continue;
                };
                let mut bound = Vec::new();
                self.binders(&matched, pattern, &mut bound);
                given.push(self.normalize(&fill(ty, &bound, 0)));
                if last.get(index).copied().unwrap_or(true) && self.covers(member, pattern) {
                    break;
                }
            }
        }
        self.union(given)
    }

    /// Whether what a type match gives depends on a type parameter:
    /// where a test meets one, or names one.
    fn undecided(&self, matching: &Matching) -> bool {
        let members = match self.unfold(&matching.subject) {
            Ty::Union(members) => members,
            other => vec![other],
        };
        let arms = matching.arms.iter();
        arms.clone().any(|(pattern, _)| names_open(pattern))
            || members.iter().any(|member| {
                arms.clone()
                    .any(|(pattern, _)| self.depends(member, pattern))
            })
    }

    /// Whether testing a value of type `ty` with `pattern` asks
    /// something of a type parameter.
    fn depends(&self, ty: &Ty, pattern: &Pat) -> bool {
        if matches!(pattern, Pattern::Wildcard) {
            return false;
        }
        if let Pattern::Bind(_, inner) = pattern {
            return self.depends(ty, inner);
        }
        let ty = self.unfold(ty);
        match (&ty, pattern) {
            (Ty::Union(members), _) => members.iter().any(|member| self.depends(member, pattern)),
            (
                Ty::Named(Reference::Parameter(_), _)
                | Ty::Bound(_)
                | Ty::Matched(_)
                | Ty::Match(_),
                _,
            ) => true,
            (_, Pattern::Is(tested)) => open(tested) || open(&ty),
            (Ty::Tuple(members), Pattern::Tuple(patterns)) => members
                .iter()
                .zip(patterns)
                .any(|(member, pattern)| self.depends(member, pattern)),
            (Ty::List(member), Pattern::Tuple(patterns)) => {
                patterns.iter().any(|pattern| self.depends(member, pattern))
            }
            (Ty::Record(fields), Pattern::Record(patterns)) => {
                patterns.iter().any(|(name, pattern)| {
                    let held = field(fields, name).cloned().unwrap_or(Ty::Unknown);
                    self.depends(&held, pattern)
                })
            }
            _ => false,
        }
    }

    /// The type of each binder in `pattern`, in binding order, where it
    /// matched `ty`.
    pub fn binders(&self, ty: &Ty, pattern: &Pat, bound: &mut Vec<Ty>) {
        match pattern {
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Is(_) => {}
            Pattern::Bind(_, inner) => {
                bound.push(ty.clone());
                self.binders(ty, inner, bound);
            }
            Pattern::Record(patterns) => {
                let ty = self.unfold(ty);
                for (name, pattern) in patterns {
                    let key = Key::Field(name.clone());
                    let member = self.field(&ty, &key).unwrap_or(Ty::Unknown);
                    self.binders(&member, pattern, bound);
                }
            }
            Pattern::Tuple(patterns) => {
                let ty = self.unfold(ty);
                for (index, pattern) in patterns.iter().enumerate() {
                    let member = self.member(&ty, index);
                    self.binders(&member, pattern, bound);
                }
            }
        }
    }

    /// The type of each binder of the arm `arm` of a match on a value
    /// of type `subject`, whose arms test `tests`: the type match that
    /// picks it out. Nothing if the subject is closed, or if the arm
    /// binds only what is left of the whole subject, since narrowing
    /// then loses nothing. A part of a type parameter is otherwise
    /// known only by its bound, which would.
    #[must_use]
    pub fn picked(&self, subject: &Ty, tests: &[Pat], arm: usize) -> Option<Vec<Ty>> {
        let pattern = tests.get(arm)?;
        let decided = !self.undecided(&TypeMatch {
            subject: subject.clone(),
            arms: vec![(pattern.clone(), Ty::Null)],
        });
        let whole = match pattern {
            Pattern::Bind(_, inner) => inner.binders() == 0,
            _ => pattern.binders() == 0,
        };
        if decided || (whole && self.covers(subject, pattern)) {
            return None;
        }
        // An earlier arm gives nothing, so what it always takes is not
        // taken again.
        let earlier = tests[..arm].iter().map(|test| (blank(test), Ty::Never));
        let each = (0..pattern.binders()).map(|binder| {
            let arms = earlier
                .clone()
                .chain([(blank(pattern), Ty::Matched(binder))])
                .collect();
            self.normalize(&Ty::Match(Box::new(TypeMatch {
                subject: subject.clone(),
                arms,
            })))
        });
        Some(each.collect())
    }

    /// A type match in normal form: its parts normalized and its
    /// binders unnamed, decided if it can be when `absorb`.
    pub(super) fn normal_match(&self, matching: &Matching, absorb: bool) -> Ty {
        let normal = TypeMatch {
            subject: self.normal(&matching.subject, absorb),
            arms: matching
                .arms
                .iter()
                .map(|(pattern, ty)| {
                    let pattern = self.normal_pattern(&blank(pattern), absorb);
                    (pattern, self.normal(ty, absorb))
                })
                .collect(),
        };
        if absorb {
            self.decide(normal)
        } else {
            Ty::Match(Box::new(normal))
        }
    }

    /// A pattern with each type it tests normalized.
    fn normal_pattern(&self, pattern: &Pat, absorb: bool) -> Pat {
        match pattern {
            Pattern::Wildcard | Pattern::Literal(_) => pattern.clone(),
            Pattern::Is(ty) => Pattern::Is(self.normal(ty, absorb)),
            Pattern::Bind(name, inner) => {
                Pattern::Bind(name.clone(), Box::new(self.normal_pattern(inner, absorb)))
            }
            Pattern::Tuple(members) => Pattern::Tuple(
                members
                    .iter()
                    .map(|member| self.normal_pattern(member, absorb))
                    .collect(),
            ),
            Pattern::Record(fields) => Pattern::Record(
                fields
                    .iter()
                    .map(|(name, inner)| (name.clone(), self.normal_pattern(inner, absorb)))
                    .collect(),
            ),
        }
    }
}

/// `pattern` with its binders unnamed: a name is no part of a type.
fn blank(pattern: &Pat) -> Pat {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Is(_) => pattern.clone(),
        Pattern::Bind(_, inner) => Pattern::Bind(String::new(), Box::new(blank(inner))),
        Pattern::Tuple(members) => Pattern::Tuple(members.iter().map(blank).collect()),
        Pattern::Record(fields) => Pattern::Record(
            fields
                .iter()
                .map(|(name, inner)| (name.clone(), blank(inner)))
                .collect(),
        ),
    }
}

/// Whether a type says nothing until a type parameter is known: it
/// names one, a parameter of a generic function type around it, or a
/// binder of an arm around it.
fn open(ty: &Ty) -> bool {
    mentions_parameter(ty) || !ty.closed() || free_matched(ty, 0)
}

/// `pattern` with each test of a type that is [`open`] made a wildcard.
fn loosened(pattern: &Pat) -> Pat {
    match pattern {
        Pattern::Is(ty) if open(ty) => Pattern::Wildcard,
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Is(_) => pattern.clone(),
        Pattern::Bind(name, inner) => Pattern::Bind(name.clone(), Box::new(loosened(inner))),
        Pattern::Tuple(members) => Pattern::Tuple(members.iter().map(loosened).collect()),
        Pattern::Record(fields) => Pattern::Record(
            fields
                .iter()
                .map(|(name, inner)| (name.clone(), loosened(inner)))
                .collect(),
        ),
    }
}

/// Whether a pattern tests for a type that is [`open`].
fn names_open(pattern: &Pat) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => false,
        Pattern::Is(ty) => open(ty),
        Pattern::Bind(_, inner) => names_open(inner),
        Pattern::Tuple(members) => members.iter().any(names_open),
        Pattern::Record(fields) => fields.iter().any(|(_, inner)| names_open(inner)),
    }
}

/// Whether `ty` names a binder of an arm outside it, given `depth`
/// binders of arms inside it that it is within.
fn free_matched(ty: &Ty, depth: usize) -> bool {
    let free = std::cell::Cell::new(false);
    let _ = ty.rewrite(0, &|ty, _| match ty {
        Ty::Matched(index) => {
            free.set(free.get() || *index >= depth);
            None
        }
        Ty::Match(matching) => {
            let outside = |ty: &Ty| free_matched(ty, depth);
            for (pattern, arm) in &matching.arms {
                let _ = pattern.retyped(0, &|ty, _| {
                    free.set(free.get() || outside(ty));
                    Some(ty.clone())
                });
                free.set(free.get() || free_matched(arm, depth + pattern.binders()));
            }
            free.set(free.get() || outside(&matching.subject));
            Some(ty.clone())
        }
        _ => None,
    });
    free.get()
}

/// `ty`, the type of an arm inside `depth` binders of arms within it,
/// with each binder of its own arm replaced by what it holds, and the
/// binders of arms around that arm renumbered past them.
fn fill(ty: &Ty, binders: &[Ty], depth: usize) -> Ty {
    ty.rewrite(0, &|ty, _| match ty {
        Ty::Matched(index) if *index < depth => Some(ty.clone()),
        Ty::Matched(index) => Some(match binders.get(index - depth) {
            Some(binder) => binder.clone(),
            None => Ty::Matched(index - binders.len()),
        }),
        Ty::Match(matching) => Some(Ty::Match(Box::new(TypeMatch {
            subject: fill(&matching.subject, binders, depth),
            arms: matching
                .arms
                .iter()
                .map(|(pattern, arm)| {
                    let pattern = pattern.retyped(0, &|ty, _| Some(fill(ty, binders, depth)));
                    let inner = depth + pattern.binders();
                    (pattern, fill(arm, binders, inner))
                })
                .collect(),
        }))),
        _ => None,
    })
}
