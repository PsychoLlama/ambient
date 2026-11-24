//! Match: what each arm matches, what it lets through, and whether
//! anything is left at the end.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Arm, Expr, Key, Pattern, Reference, is_fallback};
use crate::types::Ty;

type Pat = Pattern<Reference>;

impl Checker<'_> {
    pub(super) fn matching(
        &mut self,
        subject: &Expr<Reference>,
        arms: &[Arm<Reference>],
        expected: Option<&Ty>,
    ) -> Result<(core::Expr, Ty), Error> {
        if let [arm] = arms
            && let Pattern::Bind(_, inner) = &arm.pattern
            && let Pattern::Is(stated) = &**inner
        {
            return self.ascribed(subject, stated, &arm.body, expected);
        }
        let (subject, mut remaining) = self.infer(subject)?;
        let whole = remaining.clone();
        let mut tests = Vec::new();
        let mut core_arms = Vec::new();
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            // An arm sees only what earlier arms let through.
            let asked = self.asked(&remaining, &arm.pattern)?;
            self.decidable(&remaining, &asked)?;
            let Some(matched) = self.env.narrow(&remaining, &asked) else {
                let symbol = self.symbol.clone();
                // A fallback was written with no arm: the first is
                // never taken if the value is never null, and the
                // second if it always is.
                return Err(if is_fallback(arms) {
                    Error::NoFallback {
                        symbol,
                        always: index == 1,
                    }
                } else {
                    Error::UnreachableArm { symbol, arm: index }
                });
            };
            tests.push(asked.clone());
            // A binder of what an open type narrows to is the type match
            // that picks it out.
            let picked = self.env.picked(&whole, &tests, index).unwrap_or_default();
            let pattern = self.bind(&matched, &asked, &mut picked.into_iter());
            let (body, ty) = self.infer_to(&arm.body, expected)?;
            core_arms.push((pattern, body));
            results.push(ty);
            remaining = self.env.remainder(&remaining, &asked);
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

    /// A match whose one arm binds the subject as a type: it leaves
    /// nothing only if the subject is of that type, so nothing is
    /// tested. The subject is held to the type, which is what a `let`
    /// that states one means, and bound as it.
    fn ascribed(
        &mut self,
        subject: &Expr<Reference>,
        stated: &crate::surface::Type<Reference>,
        body: &Expr<Reference>,
        expected: Option<&Ty>,
    ) -> Result<(core::Expr, Ty), Error> {
        self.well_formed(stated)?;
        let stated = self.env.normalize(stated);
        // The core checker infers what the value is, which may be less
        // than was stated: it is told what the binder holds.
        let value = core::Expr::Widen {
            value: Box::new(self.check(subject, &stated)?),
            ty: self.lower(&stated),
        };
        self.bind_local(stated);
        let (body, ty) = self.infer_to(body, expected)?;
        let core = core::Expr::Let {
            value: Box::new(value),
            body: Box::new(body),
        };
        Ok((core, ty))
    }

    /// `pattern` as it is tested against a value of type `subject`:
    /// each type in it that no value can be tested for is the nearest
    /// one that can be. That stands only where it tells as much: what
    /// the nearer test lets through of `subject` is all of the type
    /// that was written.
    /// Refuses an arm that asks a field of a record for the type of a
    /// function in it.
    ///
    /// A function says only the type it was written as, so such a test
    /// may turn away a value of the type the arm names, and the arm is
    /// skipped with nothing said. It is reached where a record holding
    /// a function sits beside another record in the subject's type:
    /// with width subtyping a value of the other may hold the same keys,
    /// so no shape tells them apart (`{ run: fn(): int } | { done: int }`).
    /// Only an arm that takes the whole of some member of the subject's
    /// type is refused, since that member could be turned away. One that
    /// asks a function itself (`let f: fn(): int`), or asks what nothing
    /// promised (a record out of `unknown`), is left alone: asking that
    /// is what it says, and it covers nothing.
    ///
    /// TODO: this is refused only for want of time to design it, not
    /// for any reason it should be, and allowing it is wanted. Values
    /// that say their record's keys, or functions that say each type
    /// they are within, would let such an arm be decided.
    fn decidable(&self, subject: &Ty, asked: &Pat) -> Result<(), Error> {
        let members = match self.env.unfold(subject) {
            Ty::Union(members) => members,
            other => vec![other],
        };
        let promised = |member: &Ty| {
            let taken = self.env.narrow(member, asked);
            let whole = taken.is_some_and(|taken| self.env.subtype(member, &taken));
            whole && !self.env.covers(member, asked)
        };
        if !members.iter().any(promised) {
            return Ok(());
        }
        let mut tested = Vec::new();
        fields_tested(asked, false, &mut tested);
        let asks = |(within, ty): &(bool, &Ty)| {
            self.env.asks_a_name(ty) && (*within || self.holds_a_record(ty))
        };
        match tested.into_iter().find(asks).map(|(_, ty)| ty) {
            Some(ty) => Err(Error::AsksAFunction {
                symbol: self.symbol.clone(),
                found: self.env.show(ty),
            }),
            None => Ok(()),
        }
    }

    /// Whether `ty` is a record, or a union with one.
    fn holds_a_record(&self, ty: &Ty) -> bool {
        match self.env.unfold(ty) {
            Ty::Record(_) => true,
            Ty::Union(members) => members.iter().any(|member| self.holds_a_record(member)),
            _ => false,
        }
    }

    pub(super) fn asked(&self, subject: &Ty, pattern: &Pat) -> Result<Pat, Error> {
        Ok(match pattern {
            Pattern::Wildcard | Pattern::Literal(_) => pattern.clone(),
            Pattern::Bind(name, inner) => {
                Pattern::Bind(name.clone(), Box::new(self.asked(subject, inner)?))
            }
            Pattern::Is(written) => {
                self.well_formed(written)?;
                let written = self.env.normalize(written);
                // The least test that tells as much is the one made: a
                // shape, then every part with a function only asked to
                // be one, then all of the type that can be tested.
                let tells = |test: &Pat| {
                    let through = self.env.narrow(subject, test);
                    through.is_none_or(|through| self.env.subtype(&through, &written))
                };
                let shape = Pattern::Is(self.env.shape_test(&written));
                let loose = Pattern::Is(self.env.loose_test(&written));
                let near = Pattern::Is(self.env.nearest_test(&written));
                let Some(asked) = [shape, loose, near].into_iter().find(tells) else {
                    return Err(self.untestable(&written));
                };
                asked
            }
            Pattern::Tuple(members) => {
                let subject = self.env.unfold(subject);
                let asked = members
                    .iter()
                    .enumerate()
                    .map(|(index, member)| self.asked(&self.env.member(&subject, index), member));
                Pattern::Tuple(asked.collect::<Result<_, _>>()?)
            }
            Pattern::Record(fields) => {
                let subject = self.env.unfold(subject);
                let asked = fields.iter().map(|(name, field)| {
                    let key = Key::Field(name.clone());
                    let held = self.env.field(&subject, &key).unwrap_or(Ty::Unknown);
                    Ok((name.clone(), self.asked(&held, field)?))
                });
                Pattern::Record(asked.collect::<Result<_, Error>>()?)
            }
        })
    }

    /// `expr` held to `expected`, if anything is expected of it, and
    /// its type.
    pub(super) fn infer_to(
        &mut self,
        expr: &Expr<Reference>,
        expected: Option<&Ty>,
    ) -> Result<(core::Expr, Ty), Error> {
        match expected {
            Some(expected) => Ok((self.check(expr, expected)?, expected.clone())),
            None => self.infer(expr),
        }
    }

    /// Gives every binder in `pattern` its type, in binding order, and
    /// returns the core pattern. `ty` is what the pattern matched; a
    /// binder `given` holds a type for has that type instead.
    fn bind(
        &mut self,
        ty: &Ty,
        pattern: &Pat,
        given: &mut dyn Iterator<Item = Ty>,
    ) -> core::Pattern {
        // A binder holds the type by the name it had, so that it is
        // still the type definition it was, with its methods.
        let named = ty;
        let ty = self.env.unfold(ty);
        match pattern {
            Pattern::Wildcard => core::Pattern::Wildcard,
            Pattern::Literal(literal) => core::Pattern::Literal(literal.clone()),
            Pattern::Is(tested) => {
                let tested = self.env.normalize(tested);
                core::Pattern::Is(self.lower(&tested))
            }
            Pattern::Bind(_, inner) => {
                self.bind_local(given.next().unwrap_or_else(|| named.clone()));
                core::Pattern::Bind(Box::new(self.bind(named, inner, given)))
            }
            Pattern::Record(patterns) => core::Pattern::Record(
                patterns
                    .iter()
                    .map(|(name, pattern)| {
                        let key = Key::Field(name.clone());
                        let member = self.env.field(&ty, &key).unwrap_or(Ty::Unknown);
                        (name.clone(), self.bind(&member, pattern, given))
                    })
                    .collect(),
            ),
            Pattern::Tuple(patterns) => core::Pattern::Tuple(
                patterns
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        let member = self.env.member(&ty, index);
                        self.bind(&member, pattern, given)
                    })
                    .collect(),
            ),
        }
    }
}

/// The types `pattern` tests for, at any depth, each with whether it
/// is of a field of a record; `within` says whether it is inside one.
fn fields_tested<'a>(pattern: &'a Pat, within: bool, into: &mut Vec<(bool, &'a Ty)>) {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Bind(_, inner) => fields_tested(inner, within, into),
        Pattern::Is(ty) => into.push((within, ty)),
        Pattern::Tuple(members) => {
            for member in members {
                fields_tested(member, within, into);
            }
        }
        Pattern::Record(fields) => {
            for (_, field) in fields {
                fields_tested(field, true, into);
            }
        }
    }
}
