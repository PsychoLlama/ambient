//! Match: each arm sees what earlier arms let through, binds what its
//! pattern says, and nothing is left at the end.

use super::expr::Body;
use super::{Invalid, Verifier};
use crate::core::{self, Expr};
use crate::surface::{Pattern, Reference};
use crate::types::Ty;

type Pat = Pattern<Reference>;

impl Verifier<'_> {
    /// A core pattern as the relations between types read one. A binder
    /// has no name, and needs none.
    fn pattern(&self, pattern: &core::Pattern) -> Result<Pat, Invalid> {
        Ok(match pattern {
            core::Pattern::Wildcard => Pattern::Wildcard,
            core::Pattern::Literal(literal) => Pattern::Literal(literal.clone()),
            core::Pattern::Record(fields) => {
                let mut patterns: Vec<(String, Pat)> = Vec::new();
                for (name, pattern) in fields {
                    if patterns.iter().any(|(known, _)| known == name) {
                        return super::shape("a pattern names a field twice");
                    }
                    patterns.push((name.clone(), self.pattern(pattern)?));
                }
                Pattern::Record(patterns)
            }
            core::Pattern::Tuple(members) => Pattern::Tuple(
                members
                    .iter()
                    .map(|member| self.pattern(member))
                    .collect::<Result<_, _>>()?,
            ),
            core::Pattern::Is(ty) => Pattern::Is(self.tested(ty)?),
            core::Pattern::Bind(inner) => {
                Pattern::Bind(String::new(), Box::new(self.pattern(inner)?))
            }
        })
    }
}

impl Body<'_, '_> {
    pub(super) fn matching(
        &mut self,
        subject: &Expr,
        arms: &[(core::Pattern, Expr)],
    ) -> Result<Ty, Invalid> {
        let env = &self.verifier.env;
        let mut remaining = self.infer(subject)?;
        let whole = remaining.clone();
        let mut tests = Vec::new();
        let mut results = Vec::new();
        for (index, (pattern, body)) in arms.iter().enumerate() {
            let pattern = self.verifier.pattern(pattern)?;
            let matched = env.narrow(&remaining, &pattern).ok_or(Invalid::Match)?;
            tests.push(pattern.clone());
            let bound = env.picked(&whole, &tests, index).unwrap_or_else(|| {
                let mut bound = Vec::new();
                env.binders(&matched, &pattern, &mut bound);
                bound
            });
            let scope = self.locals.len();
            self.locals.extend(bound.into_iter().map(Some));
            results.push(self.infer(body)?);
            self.leave(scope);
            remaining = env.remainder(&remaining, &pattern);
        }
        if remaining == Ty::Never {
            Ok(env.union(results))
        } else {
            Err(Invalid::Match)
        }
    }
}
