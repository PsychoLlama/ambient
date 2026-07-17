//! Item lowering: functions, consts, type definitions, use flattening,
//! traits, and impls.

use std::sync::Arc;

use uuid::Uuid;

use ambient_engine::ast::{
    AbilityDef, AbilityMethod, ConstDef, EnumDef, EnumVariant, ExternFnDef, FunctionDef,
    ImplAssocType, ImplDef, ImplMethod, Item, ItemKind, Param, RowExpr, SetDef, Span, StructDef,
    TraitAssocType, TraitDef, TraitMethod, TraitRef, TypeAliasDef, TypeParam, UseDef, UsePrefix,
};
use ambient_engine::types::{NominalType, Type};

use super::LoweringContext;
use super::exprs::lower_expression;
use super::types::{lower_qualified_name, lower_type};
use crate::cst::{
    CstAbilityDef, CstConstDef, CstEnumDef, CstExternFnDef, CstFunctionDef, CstImplDef, CstItem,
    CstItemKind, CstParam, CstRowExpr, CstSetDef, CstStructDef, CstTraitDef, CstTraitParamKind,
    CstTypeAliasDef, CstTypeExprKind, CstUseDef, CstUseTree, CstUseTreeKind, CstWhereClause,
};
use crate::error::{ParseError, ParseErrorKind};

/// Lower a type parameter, carrying its trait bounds (`T: Eq + Ord`) and
/// the ability-variable flag (`E!`). An ability variable declares an effect
/// row, not a type, so it may carry no trait bounds — a bound there is a
/// clear error at the declaration site.
fn lower_type_param(tp: &crate::cst::CstTypeParam) -> Result<TypeParam, ParseError> {
    if tp.is_ability && !tp.bounds.is_empty() {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError(format!(
                "ability variable `{}` cannot have trait bounds; \
                 an ability variable names an effect row, not a type",
                tp.name.name
            )),
            tp.span,
        ));
    }
    Ok(TypeParam {
        name: tp.name.name.clone(),
        is_ability: tp.is_ability,
        bounds: tp
            .bounds
            .iter()
            .map(lower_trait_bound)
            .collect::<Result<_, _>>()?,
        span: tp.span,
    })
}

/// Lower a trait reference in bound or impl-header position: the trait's
/// qualified name plus any type arguments (`From<String>`).
fn lower_trait_bound(bound: &crate::cst::CstTraitBound) -> Result<TraitRef, ParseError> {
    Ok(TraitRef {
        name: lower_qualified_name(&bound.name),
        args: bound
            .args
            .iter()
            .map(lower_type)
            .collect::<Result<_, _>>()?,
    })
}

/// Lower a type parameter for a position that supports trait bounds but not
/// effect-polymorphic ability variables (`E!`): impl-block and trait-method
/// parameters. `E!` is wired for free functions, ability methods, and impl
/// *methods*; rejecting it here (with a position-specific `reject_message`)
/// gives a clear boundary instead of the opaque "unknown ability `E`" that an
/// unwired `with E` would otherwise produce downstream.
fn lower_type_param_no_ability(
    tp: &crate::cst::CstTypeParam,
    reject_message: &str,
) -> Result<TypeParam, ParseError> {
    if tp.is_ability {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError(reject_message.to_string()),
            tp.span,
        ));
    }
    lower_type_param(tp)
}

/// Lower a type parameter in a position where trait bounds are meaningless
/// (type declarations, which have no code to constrain, and `extern fn`s,
/// which have no dictionary calling convention). A bound there is an error
/// pointing at where bounds belong.
fn lower_unbounded_type_param(
    tp: &crate::cst::CstTypeParam,
    context: &str,
) -> Result<TypeParam, ParseError> {
    if !tp.bounds.is_empty() {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError(format!(
                "trait bounds are not supported on {context}; \
                 declare bounds on the functions, methods, and impl blocks \
                 that use the type parameter"
            )),
            tp.span,
        ));
    }
    if tp.is_ability {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError(format!(
                "ability variables are not supported on {context}; \
                 an ability variable declares a polymorphic effect row, \
                 meaningful only on functions and methods"
            )),
            tp.span,
        ));
    }
    lower_type_param(tp)
}

/// Fold trailing `where` clauses into their type parameters' bounds. `where`
/// is surface sugar with no AST channel of its own: `fn f<T>() where T: Eq`
/// lowers identically to `fn f<T: Eq>()`. A clause may only constrain one of
/// the declaration's own type parameters — there is nothing else in scope it
/// could name — and constraining anything else (a concrete type, an unknown
/// name) is a clear declaration-site error.
fn fold_where_clauses(
    type_params: &mut [TypeParam],
    where_clauses: &[CstWhereClause],
) -> Result<(), ParseError> {
    for wc in where_clauses {
        let param_name = match &wc.ty.kind {
            CstTypeExprKind::Name(name) if name.segments.len() == 1 => {
                Some(name.segments[0].name.clone())
            }
            _ => None,
        };
        let target = param_name
            .as_ref()
            .and_then(|n| type_params.iter_mut().find(|tp| &tp.name == n));
        let Some(target) = target else {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError(
                    "a `where` clause can only constrain one of the declaration's own type \
                     parameters (e.g. `fn f<T>(...) where T: Eq` or `impl<T> Wrapper<T> where \
                     T: Eq`)"
                        .into(),
                ),
                wc.span,
            ));
        };
        for bound in &wc.bounds {
            target.bounds.push(lower_trait_bound(bound)?);
        }
    }
    Ok(())
}

pub(super) fn lower_item_impl(
    ctx: &mut LoweringContext,
    item: &CstItem,
) -> Result<Vec<Item>, ParseError> {
    // Extract item documentation from doc comments (`///`)
    let doc = item.leading_trivia.extract_doc_comments().map(Arc::from);

    let kind = match &item.kind {
        CstItemKind::Function(f) => ItemKind::Function(lower_function(ctx, f)?),
        CstItemKind::Const(c) => ItemKind::Const(lower_const(ctx, c)?),
        CstItemKind::Struct(s) => ItemKind::Struct(lower_struct_def(s)?),
        CstItemKind::TypeAlias(t) => ItemKind::TypeAlias(lower_type_alias(t)?),
        CstItemKind::Set(s) => ItemKind::Set(lower_set(s)),
        CstItemKind::Enum(e) => ItemKind::Enum(lower_enum(e)?),
        CstItemKind::Ability(a) => ItemKind::Ability(lower_ability_def(ctx, a)?),
        CstItemKind::Use(u) => {
            // A use tree flattens to one item per imported leaf.
            return Ok(lower_use(u)?
                .into_iter()
                .map(|use_def| Item::with_doc(ItemKind::Use(use_def), item.span, doc.clone()))
                .collect());
        }
        CstItemKind::Trait(t) => ItemKind::Trait(lower_trait_def(t)?),
        CstItemKind::Impl(i) => ItemKind::Impl(lower_impl_def(ctx, i)?),
        CstItemKind::ExternFn(e) => ItemKind::ExternFn(lower_extern_fn(ctx, e)?),
        CstItemKind::Error => {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError("cannot lower error item".into()),
                item.span,
            ));
        }
    };

    Ok(vec![Item::with_doc(kind, item.span, doc)])
}

fn lower_function(
    ctx: &mut LoweringContext,
    f: &CstFunctionDef,
) -> Result<FunctionDef, ParseError> {
    let mut type_params = f
        .type_params
        .iter()
        .map(lower_type_param)
        .collect::<Result<Vec<_>, _>>()?;
    fold_where_clauses(&mut type_params, &f.where_clauses)?;

    let params = f
        .params
        .iter()
        .map(|p| lower_param(ctx, p))
        .collect::<Result<Vec<_>, _>>()?;

    let ret_ty = f.ret_ty.as_ref().map(lower_type).transpose()?;

    let abilities = f.abilities.iter().map(lower_qualified_name).collect();

    let body = lower_expression(ctx, &f.body)?;

    Ok(FunctionDef {
        name: f.name.name.clone(),
        name_span: f.name.span,
        is_public: f.is_public,
        type_params,
        params,
        ret_ty,
        abilities,
        body,
    })
}

/// Lower an `extern fn` declaration. There is no body to infer from, so the
/// signature must be complete: every parameter carries a type annotation and
/// the return type is declared.
fn lower_extern_fn(
    ctx: &mut LoweringContext,
    e: &CstExternFnDef,
) -> Result<ExternFnDef, ParseError> {
    let type_params = e
        .type_params
        .iter()
        .map(|tp| lower_unbounded_type_param(tp, "`extern fn` type parameters"))
        .collect::<Result<Vec<_>, _>>()?;

    let params = e
        .params
        .iter()
        .map(|p| {
            if p.ty.is_none() {
                return Err(ParseError::new(
                    ParseErrorKind::ExternFnParamRequiresType(p.name.name.to_string()),
                    p.span,
                ));
            }
            lower_param(ctx, p)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let Some(ret_ty) = e.ret_ty.as_ref().map(lower_type).transpose()? else {
        return Err(ParseError::new(
            ParseErrorKind::ExternFnRequiresReturnType,
            e.name.span,
        ));
    };

    Ok(ExternFnDef {
        name: e.name.name.clone(),
        name_span: e.name.span,
        is_public: e.is_public,
        type_params,
        params,
        ret_ty,
    })
}

pub(super) fn lower_const(
    ctx: &mut LoweringContext,
    c: &CstConstDef,
) -> Result<ConstDef, ParseError> {
    let id = ctx.fresh_binding();
    let ty = c.ty.as_ref().map(lower_type).transpose()?;
    let value = lower_expression(ctx, &c.value)?;

    Ok(ConstDef {
        id,
        name: c.name.name.clone(),
        name_span: c.name.span,
        is_public: c.is_public,
        ty,
        value,
    })
}

pub(crate) fn lower_struct_def(s: &CstStructDef) -> Result<StructDef, ParseError> {
    let type_params = s
        .type_params
        .iter()
        .map(|tp| lower_unbounded_type_param(tp, "`struct` type parameters"))
        .collect::<Result<Vec<_>, _>>()?;

    // Parse the UUID if this is a nominal (`unique(...)`) struct.
    let unique_id = s
        .unique_id
        .as_ref()
        .map(|s_uuid| {
            Uuid::parse_str(s_uuid).map_err(|e| {
                ParseError::new(ParseErrorKind::InvalidUuid(e.to_string()), s.name.span)
            })
        })
        .transpose()?;

    // An `extern` struct is engine-provided; the engine needs a stable nominal
    // identity to refer to it by, so `unique(...)` is mandatory (mirroring the
    // unit-struct rule). The parser also checks this, but a hand-built CST or a
    // recovered parse can still reach lowering without it. Checked before the
    // unit-struct rule so `extern struct T;` reports the extern-specific error.
    if s.is_extern && unique_id.is_none() {
        return Err(ParseError::new(
            ParseErrorKind::ExternStructRequiresUnique,
            s.name.span,
        ));
    }

    // Determine the record body. A unit struct (`struct Foo;`) has no body and
    // must be nominal — a fieldless structural type carries no identity and no
    // fields, mirroring the `EnumRequiresUnique` rule. A brace body must declare
    // at least one field; an empty `struct Foo {}` is rejected in favor of the
    // unit form.
    let inner_ty = match &s.ty {
        None => {
            if unique_id.is_none() {
                return Err(ParseError::new(
                    ParseErrorKind::UnitStructRequiresUnique,
                    s.name.span,
                ));
            }
            Type::Record(ambient_engine::types::RecordType { fields: vec![] })
        }
        Some(ty) => {
            if matches!(&ty.kind, CstTypeExprKind::Record(fields) if fields.is_empty()) {
                return Err(ParseError::new(
                    ParseErrorKind::EmptyStructBody,
                    s.name.span,
                ));
            }
            lower_type(ty)?
        }
    };

    // Wrap in a Nominal type when the struct carries a unique identity.
    let ty = if let Some(uuid) = unique_id {
        Type::Nominal(
            NominalType::new(uuid, inner_ty, Some(s.name.name.clone())).with_extern(s.is_extern),
        )
    } else {
        inner_ty
    };

    Ok(StructDef {
        name: s.name.name.clone(),
        name_span: s.name.span,
        is_public: s.is_public,
        type_params,
        ty,
        unique_id,
        is_extern: s.is_extern,
    })
}

fn lower_type_alias(t: &CstTypeAliasDef) -> Result<TypeAliasDef, ParseError> {
    let type_params = t
        .type_params
        .iter()
        .map(|tp| lower_unbounded_type_param(tp, "`type` alias parameters"))
        .collect::<Result<Vec<_>, _>>()?;

    let ty = lower_type(&t.ty)?;

    Ok(TypeAliasDef {
        name: t.name.name.clone(),
        name_span: t.name.span,
        is_public: t.is_public,
        type_params,
        ty,
    })
}

fn lower_set(s: &CstSetDef) -> SetDef {
    SetDef {
        name: s.name.name.clone(),
        name_span: s.name.span,
        is_public: s.is_public,
        body: lower_row_expr(&s.body),
    }
}

fn lower_row_expr(r: &CstRowExpr) -> RowExpr {
    match r {
        CstRowExpr::Name(name) => RowExpr::Name(lower_qualified_name(name)),
        CstRowExpr::Union(parts) => RowExpr::Union(parts.iter().map(lower_row_expr).collect()),
        CstRowExpr::Difference(a, b) => {
            RowExpr::Difference(Box::new(lower_row_expr(a)), Box::new(lower_row_expr(b)))
        }
    }
}

fn lower_enum(e: &CstEnumDef) -> Result<EnumDef, ParseError> {
    // Every enum is nominal: the `unique(<uuid>)` prefix is mandatory. A bare
    // `enum` has no identity, which would make structurally identical enums
    // interchangeable — the exact confusion nominal identity exists to
    // prevent — so reject it here.
    let uuid = match &e.unique_id {
        Some(s) => Uuid::parse_str(s).map_err(|err| {
            ParseError::new(ParseErrorKind::InvalidUuid(err.to_string()), e.name.span)
        })?,
        None => {
            return Err(ParseError::new(
                ParseErrorKind::EnumRequiresUnique,
                e.name.span,
            ));
        }
    };

    let type_params = e
        .type_params
        .iter()
        .map(|tp| lower_unbounded_type_param(tp, "`enum` type parameters"))
        .collect::<Result<Vec<_>, _>>()?;

    let variants = e
        .variants
        .iter()
        .map(|v| {
            let payload = v.payload.as_ref().map(lower_type).transpose()?;
            Ok(EnumVariant {
                name: v.name.name.clone(),
                payload,
                span: v.span,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(EnumDef {
        name: e.name.name.clone(),
        name_span: e.name.span,
        is_public: e.is_public,
        type_params,
        variants,
        uuid,
    })
}

fn lower_ability_def(
    ctx: &mut LoweringContext,
    a: &CstAbilityDef,
) -> Result<AbilityDef, ParseError> {
    // Abilities are nominal, like enums: the `unique(<uuid>)` prefix *is*
    // the identity, so renaming or moving the declaration never changes it.
    let uuid = match &a.unique_id {
        Some(s) => Uuid::parse_str(s).map_err(|err| {
            ParseError::new(ParseErrorKind::InvalidUuid(err.to_string()), a.name.span)
        })?,
        None => {
            return Err(ParseError::new(
                ParseErrorKind::AbilityRequiresUnique,
                a.name.span,
            ));
        }
    };

    let dependencies = a.dependencies.iter().map(lower_qualified_name).collect();

    let methods = a
        .methods
        .iter()
        .map(|m| {
            let mut type_params = m
                .type_params
                .iter()
                .map(lower_type_param)
                .collect::<Result<Vec<_>, _>>()?;
            fold_where_clauses(&mut type_params, &m.where_clauses)?;

            // A method signature is an interface other code compiles
            // against, so every parameter carries a declared type — there
            // may be no body to infer from at a handler arm.
            let params = m
                .params
                .iter()
                .map(|(name, ty)| {
                    let lowered_ty = lower_type(ty)?;
                    Ok(Param {
                        id: ctx.fresh_binding(),
                        name: name.name.clone(),
                        ty: Some(lowered_ty),
                        span: name.span,
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            let ret_ty = lower_type(&m.ret_ty)?;
            let body = m
                .body
                .as_ref()
                .map(|body| lower_expression(ctx, body))
                .transpose()?;

            Ok(AbilityMethod {
                name: m.name.name.clone(),
                name_span: m.name.span,
                type_params,
                params,
                ret_ty,
                body,
                resolved_signature: None,
                span: m.span,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(AbilityDef {
        name: a.name.name.clone(),
        name_span: a.name.span,
        is_public: a.is_public,
        dependencies,
        methods,
        uuid,
        resolved_id: None,
    })
}

/// Lower a use tree, flattening it to one `UseDef` per imported leaf.
/// Braces are pure grouping: `use a::{b, c};` lowers exactly like
/// `use a::b; use a::c;`.
pub(super) fn lower_use(u: &CstUseDef) -> Result<Vec<UseDef>, ParseError> {
    let mut out = Vec::new();
    flatten_use_tree(&u.tree, &[], false, u.is_public, &mut out)?;
    Ok(out)
}

fn flatten_use_tree(
    tree: &CstUseTree,
    base: &[crate::cst::CstIdent],
    base_leading: bool,
    is_public: bool,
    out: &mut Vec<UseDef>,
) -> Result<(), ParseError> {
    // A bare `::` roots a path at the workspace, so it is only meaningful
    // where a path begins — not on a group child that continues a path.
    if tree.leading_sep && !base.is_empty() {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError("`::` may only begin a use path".into()),
            tree.span,
        ));
    }
    let leading = base_leading || tree.leading_sep;
    let mut full: Vec<crate::cst::CstIdent> = base.to_vec();
    full.extend(tree.segments.iter().cloned());
    match &tree.kind {
        CstUseTreeKind::Leaf { alias } => {
            out.push(lower_use_leaf(
                &full,
                leading,
                alias.as_ref(),
                is_public,
                tree.span,
            )?);
        }
        CstUseTreeKind::Group(children) => {
            for child in children {
                flatten_use_tree(child, &full, leading, is_public, out)?;
            }
        }
    }
    Ok(())
}

/// Lower one flattened use path. A bare leading `::` (`leading`) roots the
/// path at the workspace ([`UsePrefix::Workspace`]: the first segment names
/// a sibling package). Otherwise the head segment determines the root: a
/// keyword (`pkg`, `core`, `self`, `super`) or a module alias from another
/// `use` (`UsePrefix::Local`). Root keywords anywhere but the head are
/// errors.
fn lower_use_leaf(
    full: &[crate::cst::CstIdent],
    leading: bool,
    alias: Option<&crate::cst::CstIdent>,
    is_public: bool,
    span: Span,
) -> Result<UseDef, ParseError> {
    let Some(head) = full.first() else {
        return Err(ParseError::new(
            ParseErrorKind::LoweringError("empty use path".into()),
            span,
        ));
    };

    let (prefix, consumed) = if leading {
        // The head is the target package's name — an ordinary identifier,
        // never a root keyword (the keyword check below covers it, since
        // nothing is consumed here).
        (UsePrefix::Workspace, 0)
    } else {
        match head.name.as_ref() {
            "pkg" => (UsePrefix::Pkg, 1),
            "core" => (UsePrefix::Core, 1),
            "self" => (UsePrefix::Self_, 1),
            "super" => {
                let supers = full
                    .iter()
                    .take_while(|seg| seg.name.as_ref() == "super")
                    .count();
                (UsePrefix::Super(supers), supers)
            }
            _ => (UsePrefix::Local, 0),
        }
    };

    for seg in &full[consumed..] {
        if matches!(seg.name.as_ref(), "pkg" | "core" | "self" | "super") {
            return Err(ParseError::new(
                ParseErrorKind::LoweringError(format!("`{}` may only begin a use path", seg.name)),
                seg.span,
            ));
        }
    }

    Ok(UseDef {
        is_public,
        prefix,
        path: full[consumed..]
            .iter()
            .map(|seg| (seg.name.clone(), seg.span))
            .collect(),
        alias: alias.map(|a| (a.name.clone(), a.span)),
    })
}

pub(super) fn lower_param(ctx: &mut LoweringContext, p: &CstParam) -> Result<Param, ParseError> {
    let id = ctx.fresh_binding();
    let ty = p.ty.as_ref().map(lower_type).transpose()?;

    Ok(Param {
        id,
        name: p.name.name.clone(),
        ty,
        span: p.span,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait and Impl Lowering
// ─────────────────────────────────────────────────────────────────────────────

fn lower_trait_def(t: &CstTraitDef) -> Result<TraitDef, ParseError> {
    // Traits are nominal, like enums and abilities: the `unique(<uuid>)`
    // prefix *is* the identity that bounds, impls, and dispatch key off.
    let uuid = match &t.unique_id {
        Some(s) => Uuid::parse_str(s).map_err(|err| {
            ParseError::new(ParseErrorKind::InvalidUuid(err.to_string()), t.name.span)
        })?,
        None => {
            return Err(ParseError::new(
                ParseErrorKind::TraitRequiresUnique,
                t.name.span,
            ));
        }
    };

    // Trait-level type parameters (generic traits) and supertraits are not
    // supported yet, but they lower permissively so the checker can reject them
    // with a single clear declaration-site diagnostic (rather than an opaque
    // parse error) — see `register_traits`.
    let type_params = t
        .type_params
        .iter()
        .map(lower_type_param)
        .collect::<Result<Vec<_>, _>>()?;

    let supertraits = t.supertraits.iter().map(lower_qualified_name).collect();

    let assoc_types = t
        .assoc_types
        .iter()
        .map(|a| TraitAssocType {
            name: a.name.name.clone(),
            name_span: a.name.span,
            span: a.span,
        })
        .collect();

    let methods = t
        .methods
        .iter()
        .map(|m| {
            // A trait method is a wired position for both effect polymorphism
            // (`E!`) and trait bounds (`<U: Eq>`): a concrete-receiver call
            // threads one hidden dictionary per bound as a trailing argument,
            // exactly like a bounded free function.
            let mut method_type_params = m
                .type_params
                .iter()
                .map(lower_type_param)
                .collect::<Result<Vec<_>, _>>()?;
            fold_where_clauses(&mut method_type_params, &m.where_clauses)?;

            // Check if first param is self
            let (has_self, other_params) = if let Some(first) = m.params.first() {
                match &first.kind {
                    CstTraitParamKind::SelfParam => (true, &m.params[1..]),
                    CstTraitParamKind::Named { .. } => (false, &m.params[..]),
                }
            } else {
                (false, &m.params[..])
            };

            let params = other_params
                .iter()
                .map(|p| match &p.kind {
                    CstTraitParamKind::Named { name, ty } => {
                        let lowered_ty = lower_type(ty)?;
                        Ok((name.name.clone(), lowered_ty))
                    }
                    CstTraitParamKind::SelfParam => Err(ParseError::new(
                        ParseErrorKind::LoweringError(
                            "self can only be the first parameter".into(),
                        ),
                        p.span,
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;

            let ret_ty = lower_type(&m.ret_ty)?;

            let abilities = m.abilities.iter().map(lower_qualified_name).collect();

            Ok(TraitMethod {
                name: m.name.name.clone(),
                name_span: m.name.span,
                type_params: method_type_params,
                has_self,
                params,
                ret_ty,
                abilities,
                span: m.span,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TraitDef {
        name: t.name.name.clone(),
        name_span: t.name.span,
        is_public: t.is_public,
        uuid,
        type_params,
        supertraits,
        assoc_types,
        methods,
    })
}

fn lower_impl_def(ctx: &mut LoweringContext, i: &CstImplDef) -> Result<ImplDef, ParseError> {
    let mut type_params: Vec<TypeParam> = i
        .type_params
        .iter()
        .map(|tp| {
            lower_type_param_no_ability(
                tp,
                "ability variables (`E!`) are not supported on impl blocks; an \
                 impl block's type parameters parameterize the receiver type, \
                 where an effect row cannot appear. Declare `E!` on the method \
                 instead: `fn method<E!>(...)`",
            )
        })
        .collect::<Result<_, _>>()?;

    let trait_name = i.trait_name.as_ref().map(lower_trait_bound).transpose()?;
    let for_type = lower_type(&i.for_type)?;

    // `where T: Bound` is the trailing spelling of an inline bound: fold each
    // clause into its type parameter so bounds have exactly one AST
    // representation.
    fold_where_clauses(&mut type_params, &i.where_clauses)?;

    let assoc_types = i
        .assoc_types
        .iter()
        .map(|a| {
            Ok(ImplAssocType {
                name: a.name.name.clone(),
                name_span: a.name.span,
                ty: lower_type(&a.ty)?,
                span: a.span,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    let methods = i
        .methods
        .iter()
        .map(|m| {
            // Impl methods are a wired position for effect polymorphism:
            // `fn each<E!>(self, f: (T) -> () with E): () with E` accepts and
            // propagates `E!` exactly like a free function.
            let mut method_type_params = m
                .type_params
                .iter()
                .map(lower_type_param)
                .collect::<Result<Vec<_>, _>>()?;
            fold_where_clauses(&mut method_type_params, &m.where_clauses)?;

            // Allocate self binding ID
            let self_id = ctx.fresh_binding();

            // A leading `self` parameter marks an instance method; its
            // absence marks an associated method (e.g. `Default::default`).
            let has_self = m
                .params
                .first()
                .is_some_and(|p| matches!(p.kind, CstTraitParamKind::SelfParam));

            // Lower non-self parameters
            let params = m
                .params
                .iter()
                .filter_map(|p| match &p.kind {
                    CstTraitParamKind::SelfParam => None,
                    CstTraitParamKind::Named { name, ty } => Some({
                        let lowered_ty = lower_type(ty).ok();
                        Ok(Param {
                            id: ctx.fresh_binding(),
                            name: name.name.clone(),
                            ty: lowered_ty,
                            span: p.span,
                        })
                    }),
                })
                .collect::<Result<Vec<_>, ParseError>>()?;

            let ret_ty = m.ret_ty.as_ref().map(lower_type).transpose()?;
            let abilities = m.abilities.iter().map(lower_qualified_name).collect();
            let body = lower_expression(ctx, &m.body)?;

            Ok(ImplMethod {
                name: m.name.name.clone(),
                name_span: m.name.span,
                type_params: method_type_params,
                has_self,
                self_id,
                params,
                ret_ty,
                abilities,
                body,
                span: m.span,
                resolved_symbol: None,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ImplDef {
        type_params,
        trait_name,
        for_type,
        assoc_types,
        methods,
        span: i.span,
    })
}
