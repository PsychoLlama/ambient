//! Match: what each arm matches, what it lets through, and whether
//! anything is left at the end.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Arm, Expr, Key, Pattern, Reference};
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
        let mut core_arms = Vec::new();
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            // An arm sees only what earlier arms let through.
            let asked = self.asked(&remaining, &arm.pattern)?;
            let Some(matched) = self.env.narrow(&remaining, &asked) else {
                return Err(Error::UnreachableArm {
                    symbol: self.symbol.clone(),
                    arm: index,
                });
            };
            let pattern = self.bind(&matched, &asked);
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
    pub(super) fn asked(&self, subject: &Ty, pattern: &Pat) -> Result<Pat, Error> {
        Ok(match pattern {
            Pattern::Wildcard | Pattern::Literal(_) => pattern.clone(),
            Pattern::Bind(name, inner) => {
                Pattern::Bind(name.clone(), Box::new(self.asked(subject, inner)?))
            }
            Pattern::Is(written) => {
                self.well_formed(written)?;
                let written = self.env.normalize(written);
                let near = Pattern::Is(self.env.nearest_test(&written));
                let through = self.env.narrow(subject, &near);
                if through.is_some_and(|through| !self.env.subtype(&through, &written)) {
                    return Err(Error::Untestable {
                        symbol: self.symbol.clone(),
                        found: self.env.show(&written),
                    });
                }
                near
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
    /// returns the core pattern. `ty` is what the pattern matched.
    pub(super) fn bind(&mut self, ty: &Ty, pattern: &Pat) -> core::Pattern {
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
                self.bind_local(named.clone());
                core::Pattern::Bind(Box::new(self.bind(named, inner)))
            }
            Pattern::Record(patterns) => core::Pattern::Record(
                patterns
                    .iter()
                    .map(|(name, pattern)| {
                        let key = Key::Field(name.clone());
                        let member = self.env.field(&ty, &key).unwrap_or(Ty::Unknown);
                        (name.clone(), self.bind(&member, pattern))
                    })
                    .collect(),
            ),
            Pattern::Tuple(patterns) => core::Pattern::Tuple(
                patterns
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        let member = self.env.member(&ty, index);
                        self.bind(&member, pattern)
                    })
                    .collect(),
            ),
        }
    }
}
