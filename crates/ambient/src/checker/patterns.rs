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
            let Some(matched) = self.env.narrow(&remaining, &arm.pattern) else {
                return Err(Error::UnreachableArm {
                    symbol: self.symbol.clone(),
                    arm: index,
                });
            };
            let pattern = self.bind(&matched, &arm.pattern);
            let (body, ty) = self.infer(&arm.body)?;
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
