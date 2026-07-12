//! Type-expression and qualified-name lowering.

use std::sync::Arc;

use ambient_engine::ast::{QualifiedName, Span};
use ambient_engine::types::Type;

use crate::cst::{CstQualifiedName, CstTypeExpr, CstTypeExprKind};
use crate::error::{ParseError, ParseErrorKind};

/// Lower a CST type expression to an AST Type.
///
/// # Panics
/// Uses `expect()` for qualified name segments which is safe because qualified
/// names always have at least one segment by parser construction.
#[allow(clippy::expect_used, clippy::too_many_lines)]
pub(super) fn lower_type(ty: &CstTypeExpr) -> Result<Type, ParseError> {
    match &ty.kind {
        CstTypeExprKind::Name(qn) => {
            let name = type_name_from_segments(qn);
            match &*name {
                // The old lowercase spellings are gone; point at the fix.
                // Primitives themselves (`Number`/`String`/`Bool`/`Binary`) are
                // no longer intercepted here — they fall through to the generic
                // `Named { uuid: None }` arm and resolve via the prelude alias,
                // like any other declared type.
                "number" | "string" | "bool" | "binary" => {
                    let suggestion = match &*name {
                        "number" => "Number",
                        "string" => "String",
                        "binary" => "Binary",
                        _ => "Bool",
                    };
                    Err(ParseError::new(
                        ParseErrorKind::Expected {
                            expected: format!("`{suggestion}`"),
                            found: format!("`{name}` (primitive types are PascalCase)"),
                        },
                        ty.span,
                    ))
                }
                _ => {
                    // Named type - could be generic, user-defined, etc.
                    // For now, represent as a named type
                    Ok(Type::Named(ambient_engine::types::NamedType {
                        name,
                        args: Vec::new(),
                        uuid: None,
                    }))
                }
            }
        }

        CstTypeExprKind::Generic { base, args } => {
            // Extract base name
            let base_name = match &base.kind {
                CstTypeExprKind::Name(qn) => type_name_from_segments(qn),
                _ => {
                    return Err(ParseError::new(ParseErrorKind::InvalidType, base.span));
                }
            };

            let lowered_args = args.iter().map(lower_type).collect::<Result<Vec<_>, _>>()?;

            Ok(Type::Named(ambient_engine::types::NamedType {
                name: base_name,
                args: lowered_args,
                uuid: None,
            }))
        }

        CstTypeExprKind::Tuple(elements) => {
            if elements.is_empty() {
                Ok(Type::Unit)
            } else {
                let lowered = elements
                    .iter()
                    .map(lower_type)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Type::Tuple(lowered))
            }
        }

        CstTypeExprKind::Record(fields) => {
            let lowered_fields = fields
                .iter()
                .map(|(name, ty)| {
                    let lowered_ty = lower_type(ty)?;
                    Ok((name.name.clone(), lowered_ty))
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            Ok(Type::Record(ambient_engine::types::RecordType {
                fields: lowered_fields.into_iter().collect(),
            }))
        }

        CstTypeExprKind::Function {
            params,
            ret,
            abilities,
        } => {
            let param_types = params
                .iter()
                .map(lower_type)
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = lower_type(ret)?;

            // Ability names can't be resolved to IDs here (lowering has no
            // ability resolver); pass them through symbolically for the type
            // checker's resolve_holes to resolve. Qualified names keep their
            // full `::`-joined spelling (`core::system::Stdio`) so the checker
            // can enforce the namespace policy on effect rows too.
            let ability_set = if abilities.is_empty() {
                ambient_engine::types::AbilitySet::empty()
            } else {
                ambient_engine::types::AbilitySet::Unresolved(
                    abilities
                        .iter()
                        .map(|qn| {
                            let segments: Vec<&str> =
                                qn.segments.iter().map(|seg| seg.name.as_ref()).collect();
                            std::sync::Arc::from(segments.join("::"))
                        })
                        .collect(),
                )
            };

            Ok(Type::Function(ambient_engine::types::FunctionType {
                params: param_types,
                ret: Box::new(return_type),
                abilities: ability_set,
            }))
        }

        // `Handler<A>` / `Handler<A, R>`: a dedicated node, not a nominal
        // name. `A` is an ability reference (`::`-joined if qualified),
        // resolved by the checker through the ability namespace; `R` the
        // optional answer type.
        CstTypeExprKind::Handler { ability, answer } => {
            let answer = answer
                .as_ref()
                .map(|ty| lower_type(ty).map(Box::new))
                .transpose()?;
            Ok(Type::HandlerAnnotation(
                ambient_engine::types::HandlerAnnotationType {
                    ability: type_name_from_segments(ability),
                    answer,
                },
            ))
        }

        // The never type `!` lowers straight to `Type::Never` — the checker's
        // canonical bottom type. (An ability method returning `!`, such as
        // `Exception::throw`, must render `never` for its signature hash and
        // must map to `None` resume-value type; a stringly `Named("!")` would
        // do neither.)
        CstTypeExprKind::Never => Ok(Type::Never),

        CstTypeExprKind::Infer => {
            // Inferred type - the type checker will fill this in
            // For now, return a type variable placeholder
            Ok(Type::Var(0))
        }

        CstTypeExprKind::Error => Err(ParseError::new(
            ParseErrorKind::LoweringError("cannot lower error type".into()),
            ty.span,
        )),
    }
}

/// The qualified name a type reference lowers to: the bare last segment
/// for an unqualified reference, the full qualified path (`pkg::types::Money`,
/// `core::time::Duration`) for a qualified one. The resolve pass
/// (`ambient_engine::resolve`) rewrites qualified type names to their
/// canonical target.
fn type_name_from_segments(qn: &CstQualifiedName) -> Arc<str> {
    if qn.segments.len() == 1 {
        qn.segments[0].name.clone()
    } else {
        qn.segments
            .iter()
            .map(|seg| seg.name.as_ref())
            .collect::<Vec<_>>()
            .join("::")
            .into()
    }
}

/// Lower a CST qualified name to an AST `QualifiedName`.
///
/// # Panics
/// Uses `expect()` for segment access which is safe because qualified names
/// always have at least one segment by parser construction.
#[allow(clippy::expect_used)]
pub(super) fn lower_qualified_name(qn: &CstQualifiedName) -> QualifiedName {
    if qn.segments.len() == 1 {
        let seg = &qn.segments[0];
        QualifiedName {
            path: Vec::new(),
            path_spans: Vec::new(),
            name: seg.name.clone(),
            name_span: Some(seg.span),
            resolved: None,
        }
    } else {
        let name_seg = qn.segments.last().expect("segments not empty");
        let path: Vec<Arc<str>> = qn.segments[..qn.segments.len() - 1]
            .iter()
            .map(|i| i.name.clone())
            .collect();
        let path_spans: Vec<Span> = qn.segments[..qn.segments.len() - 1]
            .iter()
            .map(|i| i.span)
            .collect();
        QualifiedName {
            path,
            path_spans,
            name: name_seg.name.clone(),
            name_span: Some(name_seg.span),
            resolved: None,
        }
    }
}
