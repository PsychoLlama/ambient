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

use super::error::TypeErrorKind;
use super::{Infer, InferResult, TypeEnv, type_error};
use crate::ast::{Pattern, PatternKind};
use crate::types::Type;
use std::sync::Arc;

impl Infer {
    /// Infer types for a pattern and return the environment extended with
    /// its bindings.
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
        let mut new_env = env.extend();
        self.bind_pattern(&mut new_env, pattern, expected)?;
        Ok(new_env)
    }

    /// Check a pattern against the expected type, inserting its bindings
    /// into `env` directly (nested patterns share the one environment
    /// rather than merging copies).
    fn bind_pattern(
        &mut self,
        env: &mut TypeEnv,
        pattern: &Pattern,
        expected: &Type,
    ) -> InferResult<()> {
        let span = (pattern.span.start, pattern.span.end);

        match &pattern.kind {
            PatternKind::Wildcard => {
                // Wildcard matches anything
            }

            PatternKind::Variant(variant_name, inner) => {
                // Resolved-first: the resolve pass stamps a variant pattern
                // with its two-segment ident `Fqn(module, [Enum, Variant])`,
                // so pick the enum by that ident's first segment and the
                // variant by its second — a variant name shared by two enums
                // never mis-dispatches. A registry-less check leaves
                // `resolved` `None` and falls back to the bare-name reverse
                // lookup (which the resolved path deliberately avoids).
                let resolved = variant_name.resolved.as_ref().and_then(|fqn| {
                    let enum_name = fqn.ident.first()?;
                    let variant = fqn.ident.get(1)?;
                    let info = self.enum_registry.get(enum_name)?;
                    let idx = info.variants.iter().position(|v| v.name == *variant)?;
                    Some((Arc::clone(info), idx))
                });
                let Some((info, idx)) =
                    resolved.or_else(|| self.enum_registry.resolve_variant(&variant_name.name))
                else {
                    return Err(type_error(
                        TypeErrorKind::UnknownVariant {
                            name: Arc::clone(&variant_name.name),
                        },
                        span,
                    ));
                };

                let (enum_ty, payload_ty) = info.instantiate_variant(self, idx);
                self.unify(expected, &enum_ty, span)?;

                match (payload_ty, inner) {
                    (Some(payload), Some(inner_pat)) => {
                        self.bind_pattern(env, inner_pat, &payload)?;
                    }
                    (None, None) => {}
                    (expects_payload, _) => {
                        return Err(type_error(
                            TypeErrorKind::VariantPayloadMismatch {
                                variant: Arc::clone(&variant_name.name),
                                expects_payload: expects_payload.is_some(),
                            },
                            span,
                        ));
                    }
                }
            }

            PatternKind::Binding(id, name) => {
                env.insert_mono(*id, name.clone(), expected.clone());
            }

            PatternKind::Literal(lit) => {
                let lit_ty = match lit {
                    crate::ast::Literal::Unit => Type::Unit,
                    crate::ast::Literal::Bool(_) => Type::bool(),
                    crate::ast::Literal::Number(_) => Type::number(),
                    crate::ast::Literal::String(_) => Type::string(),
                };
                self.unify(expected, &lit_ty, span)?;
            }

            PatternKind::Tuple(patterns) => {
                let elem_tys: Vec<_> = (0..patterns.len()).map(|_| self.fresh()).collect();
                let tuple_ty = Type::Tuple(elem_tys.clone());
                self.unify(expected, &tuple_ty, span)?;

                for (pat, ty) in patterns.iter().zip(elem_tys.iter()) {
                    self.bind_pattern(env, pat, ty)?;
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
                    self.bind_pattern(env, pat, ty)?;
                }
            }
        }

        Ok(())
    }
}
