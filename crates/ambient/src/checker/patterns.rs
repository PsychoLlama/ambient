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
        for arm in arms {
            let mut result = Ok(());
            pattern_types(&arm.pattern, &mut |ty| {
                if result.is_ok() {
                    result = self.tested(ty);
                }
            });
            result?;
        }
        let mut core_arms = Vec::new();
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            // An arm sees only what earlier arms let through.
            let Some(matched) = self.env.narrow(&remaining, &arm.pattern) else {
                return Err(Error::UnreachableArm {
                    symbol: self.symbol.clone(),
                    arm: index,
                });
            };
            let pattern = self.bind(&matched, &arm.pattern);
            let (body, ty) = self.infer_to(&arm.body, expected)?;
            core_arms.push((pattern, body));
            results.push(ty);
            remaining = self.env.remainder(&remaining, &arm.pattern);
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
        let value = self.check(subject, &stated)?;
        self.bind_local(stated);
        let (body, ty) = self.infer_to(body, expected)?;
        let core = core::Expr::Let {
            value: Box::new(value),
            body: Box::new(body),
        };
        Ok((core, ty))
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
