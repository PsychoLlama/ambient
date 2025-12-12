//! Pattern matching type inference.
//!
//! This module handles type inference for pattern matching constructs,
//! including:
//! - Wildcard patterns (`_`)
//! - Binding patterns (variable capture)
//! - Literal patterns (unit, bool, number, string)
//! - Tuple patterns
//! - Record patterns
//! - Variant patterns (enum destructuring)

use super::{Infer, InferResult, TypeEnv};
use crate::ast::{Pattern, PatternKind};
use crate::types::Type;

impl Infer {
    /// Infer types for a pattern and return extended environment.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if the pattern doesn't match the expected type.
    pub fn infer_pattern(
        &mut self,
        env: &TypeEnv,
        pattern: &Pattern,
        expected: &Type,
    ) -> InferResult<TypeEnv> {
        let span = (pattern.span.start, pattern.span.end);
        let mut new_env = env.extend();

        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Variant(_, _) => {
                // Wildcard matches anything
                // Variant patterns require enum type definitions (future work)
            }

            PatternKind::Binding(id, name) => {
                new_env.insert_mono(*id, name.clone(), expected.clone());
            }

            PatternKind::Literal(lit) => {
                let lit_ty = match lit {
                    crate::ast::Literal::Unit => Type::Unit,
                    crate::ast::Literal::Bool(_) => Type::Bool,
                    crate::ast::Literal::Number(_) => Type::Number,
                    crate::ast::Literal::String(_) => Type::String,
                };
                self.unify(expected, &lit_ty, span)?;
            }

            PatternKind::Tuple(patterns) => {
                let elem_tys: Vec<_> = (0..patterns.len()).map(|_| self.fresh()).collect();
                let tuple_ty = Type::Tuple(elem_tys.clone());
                self.unify(expected, &tuple_ty, span)?;

                for (pat, ty) in patterns.iter().zip(elem_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    // Merge bindings
                    for (id, name, scheme) in pat_env.iter_named() {
                        new_env.insert(id, name.clone(), scheme.clone());
                    }
                }
            }

            PatternKind::Record(field_patterns) => {
                let mut field_tys = Vec::with_capacity(field_patterns.len());
                for (name, _) in field_patterns {
                    field_tys.push((name.clone(), self.fresh()));
                }
                let record_ty = Type::record(field_tys.clone());
                self.unify(expected, &record_ty, span)?;

                for ((_, pat), (_, ty)) in field_patterns.iter().zip(field_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    for (id, name, scheme) in pat_env.iter_named() {
                        new_env.insert(id, name.clone(), scheme.clone());
                    }
                }
            }
        }

        Ok(new_env)
    }
}
