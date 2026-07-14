//! The remaining reference forms: variant patterns, scoped variant
//! paths, module aliases, trait bounds, `use` imports (module-level and
//! block-scoped), bare type annotations, and fully-qualified performs.
//! Several forms record *no* resolve-pass dep — each pin explains why.

use super::*;

// ─────────────────────────────────────────────────────────────────────────

#[test]
fn imported_variant_pattern_is_a_link_dep() {
    // `use pkg::defs::Circle; match … { Circle => () }`. A variant *pattern*
    // names the variant through the same `resolve_value_ref` machinery as a
    // construction expression (see `resolve_pattern` in `walk`), so it lands
    // on the same classification: a link-order edge, both sets.
    let (outcome, defs) = resolve_main(
        vec![enum_item("Shape", "Circle", 0x57)],
        vec![
            use_from_defs("Circle"),
            func(
                "g",
                match_expr(variant_pattern(QualifiedName::simple("Circle"))),
                vec![],
            ),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        outcome.link_deps.contains(&defs),
        "a variant pattern shares the value-position classification"
    );
    assert_subset(&outcome);
}

#[test]
fn qualified_variant_pattern_is_a_link_dep() {
    // `match … { pkg::defs::Shape::Circle => () }` (no `use`) — the final
    // *path* segment names an enum, so `resolve_module_prefix` fails and the
    // reference is rescued by `resolve_explicit_enum_variant`, always a
    // value/construction site: both sets.
    let (outcome, defs) = resolve_main(
        vec![enum_item("Shape", "Circle", 0x58)],
        vec![func(
            "g",
            match_expr(variant_pattern(QualifiedName::qualified(
                vec!["pkg", "defs", "Shape"],
                "Circle",
            ))),
            vec![],
        )],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn scoped_variant_pattern_via_imported_enum_is_a_link_dep() {
    // `use pkg::defs::Shape; match … { Shape::Circle => () }` — the empty
    // module prefix routes through `resolve_scoped_enum_variant`, which
    // canonicalizes into the *enum's defining module*: both sets.
    let (outcome, defs) = resolve_main(
        vec![enum_item("Shape", "Circle", 0x59)],
        vec![
            use_from_defs("Shape"),
            func(
                "g",
                match_expr(variant_pattern(QualifiedName::qualified(
                    vec!["Shape"],
                    "Circle",
                ))),
                vec![],
            ),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn scoped_variant_construction_via_imported_enum_is_a_link_dep() {
    // The expression twin of the pattern above: `use pkg::defs::Shape;
    // Shape::Circle` in value position, also `resolve_scoped_enum_variant`.
    let (outcome, defs) = resolve_main(
        vec![enum_item("Shape", "Circle", 0x5A)],
        vec![
            use_from_defs("Shape"),
            func(
                "g",
                name_expr(QualifiedName::qualified(vec!["Shape"], "Circle")),
                vec![],
            ),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// Module aliases
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn module_alias_method_call_is_a_link_dep() {
    // `use pkg::defs; defs.helper()` — parsed as a method call on the value
    // `defs`, rewritten to a qualified call by the module-alias
    // disambiguation in `walk::resolve_expr`, which canonicalizes through
    // `canonical_value`: both sets.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![
            use_defs_module(),
            func("g", method_call_expr("defs", "helper"), vec![]),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        outcome.link_deps.contains(&defs),
        "the module-alias method-call rewrite is a qualified call"
    );
    assert_subset(&outcome);
}

#[test]
fn unreferenced_module_alias_records_no_dep() {
    // `use pkg::defs;` alone. A *module* import binds an alias in
    // `ModuleScope::modules`, not an item in `ModuleScope::items`, and the
    // `use`-import loop in `resolve_module` iterates items only — so an
    // unreferenced module alias records *no* dep edge at all. Its target's
    // existence is validated when the scope is built, and every actual
    // reference through the alias records its own edge, so nothing is lost;
    // this pins that a bare module alias is not itself a dependency.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![use_defs_module()],
    );
    assert!(
        !outcome.deps.contains(&defs),
        "an unreferenced module alias is not a dep"
    );
    assert!(!outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// Trait bounds
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn qualified_trait_bound_records_no_dep() {
    // `fn f<T: pkg::defs::Describe>()` (no `use`). `resolve_trait_ref`
    // canonicalizes the bound to the trait's `Fqn` but deliberately records
    // *no* dep edge of either kind: a bound needs only the trait's
    // *definition*, which is registered upfront for every module before any
    // compilation, never its compiled body — a dep edge here would
    // manufacture spurious compile-order cycles (see `resolve_trait_ref`).
    // (An imported bound's defining module still enters `deps` through its
    // `use` item, exercised by the impl-header test above.)
    let (outcome, defs, main) = resolve_main_module(
        vec![trait_item("Describe", 0x5B)],
        vec![bounded_func(
            "f",
            QualifiedName::qualified(vec!["pkg", "defs"], "Describe"),
        )],
    );
    // Guard against a vacuous pass: the bound must actually have resolved.
    let bound_fqn = main.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) => f.type_params[0].bounds[0].resolved.clone(),
        _ => None,
    });
    assert_eq!(
        bound_fqn,
        Some(crate::fqn::Fqn::new(
            defs.clone(),
            vec![Arc::from("Describe")]
        )),
        "the bound resolved to the trait's Fqn"
    );
    assert!(
        !outcome.deps.contains(&defs),
        "a trait bound records no compile-ordering dep"
    );
    assert!(!outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// `use` imports and type annotations
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn unreferenced_item_import_is_check_only() {
    // `use pkg::defs::helper;` with no reference. The `use`-import loop in
    // `resolve_module` records the edge through the funnel as
    // `RefPos::Import` — check-only, `deps` alone: the import's target must
    // exist for the module to check, but a `use` emits nothing at compile
    // time.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![use_from_defs("helper")],
    );
    assert!(
        outcome.deps.contains(&defs),
        "an unreferenced item import is still a check-order dep"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "a use statement emits no link artifact"
    );
    assert_subset(&outcome);
}

#[test]
fn bare_imported_type_annotation_dep_comes_from_the_use_alone() {
    // `use pkg::defs::Widget; fn f(x: Widget)`. The *bare* annotation head
    // resolves through `types::resolve_named_type_head` →
    // `apply_named_type_from_module`, which records no dep at all (a bare
    // type reference needs only the defining module registered — recording
    // one would manufacture spurious cycles, e.g. core's `convert`/`string`
    // mutual type references). The `deps` edge below comes exclusively from
    // the `use` item (`RefPos::Import`). This differs from the *dotted*
    // spelling (`x: pkg::defs::Widget`), which records a check-only edge in
    // `resolve_type`'s qualified arm — see
    // `qualified_type_annotation_is_check_only`.
    let (outcome, defs) = resolve_main(
        vec![record_struct("Widget")],
        vec![
            use_from_defs("Widget"),
            func_with_param("f", named("Widget")),
        ],
    );
    assert!(outcome.deps.contains(&defs), "the use records the edge");
    assert!(
        !outcome.link_deps.contains(&defs),
        "neither the use nor the bare annotation is a link edge"
    );
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// Block-scoped `use`
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn block_scoped_use_with_value_reference_is_a_link_dep() {
    // `{ use pkg::defs::helper; helper }` — the block `use` binds an overlay
    // consumed by `resolve_value_ref`, and the *reference* records the
    // link-order edge through `canonical_value` exactly like a module-level
    // import: both sets.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![func(
            "g",
            block_expr(
                vec![use_from_defs_stmt("helper")],
                Some(Expr::name("helper")),
            ),
            vec![],
        )],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn unreferenced_block_scoped_use_is_check_only() {
    // `{ use pkg::defs::helper; }` with no reference. A `use` is a dependency
    // at *any* scope: `bind_block_use` records a check-only `RefPos::Import`
    // edge for every foreign item the block `use` binds, exactly like the
    // module-level import loop, even when the binding is never referenced. So
    // the edge lands in `deps` but not `link_deps` — a `use` emits nothing at
    // compile time, and no value reference through the overlay exists to add a
    // link edge.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![func(
            "g",
            block_expr(vec![use_from_defs_stmt("helper")], None),
            vec![],
        )],
    );
    assert!(
        outcome.deps.contains(&defs),
        "an unreferenced block-scoped use is still a check-order dep"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "a use statement emits no link artifact"
    );
    assert_subset(&outcome);
}

#[test]
fn block_scoped_use_with_type_only_reference_is_check_only() {
    // `{ use pkg::defs::Widget; let x: Widget = 1; }`. The bare annotation
    // head resolves through `resolve_named_type_head`, which records no dep of
    // its own — but the block `use` itself now records a check-only
    // `RefPos::Import` edge (`bind_block_use`), so the type's defining module
    // does enter `deps`. That edge is what carries a block-imported
    // definition's interface change into the consumer's cache key: even though
    // `apply_named_type_from_module` inlines the identity into the annotation
    // during resolve today, the `use`-is-a-dependency rule makes the
    // invalidation structural rather than reliant on that inlining staying
    // total. It stays out of `link_deps`: a `use` emits no link artifact.
    let (outcome, defs, main) = resolve_main_module(
        vec![record_struct("Widget")],
        vec![func(
            "g",
            block_expr(
                vec![
                    use_from_defs_stmt("Widget"),
                    Stmt::new(
                        StmtKind::Let(LetBinding {
                            id: 0,
                            name: Arc::from("x"),
                            name_span: Span::default(),
                            ty: Some(named("Widget")),
                            init: Expr::number(1.0),
                        }),
                        Span::default(),
                    ),
                ],
                None,
            ),
            vec![],
        )],
    );
    // Guard against a vacuous pass: the annotation must actually have been
    // rewritten to the struct's record type by `apply_named_type_from_module`.
    let let_ty = main.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) => match &f.body.kind {
            ExprKind::Block(stmts, _) => stmts.iter().find_map(|stmt| match &stmt.kind {
                StmtKind::Let(binding) => binding.ty.clone(),
                _ => None,
            }),
            _ => None,
        },
        _ => None,
    });
    assert!(
        matches!(let_ty, Some(Type::Record(_))),
        "the annotation resolved through the block use: {let_ty:?}"
    );
    assert!(
        outcome.deps.contains(&defs),
        "a type-only block-scoped use records a check-order dep (its interface \
         change must reach the consumer's cache key)"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "a use statement emits no link artifact"
    );
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// Fully-qualified performs
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn fully_qualified_perform_is_a_link_dep() {
    // `pkg::defs::Log::log!()` (no `use`) — the qualified ability spelling
    // routes through `resolve_path_ref(RefPos::Value)` / `lookup_item`
    // rather than the bare-import arm of `resolve_ability_ref`, but lands on
    // the same classification: a perform links the ability's dispatch
    // channel, both sets.
    let (outcome, defs) = resolve_main(
        vec![ability_item("Log", 0x5C)],
        vec![func(
            "g",
            perform_expr(QualifiedName::qualified(vec!["pkg", "defs"], "Log"), "log"),
            vec![],
        )],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        outcome.link_deps.contains(&defs),
        "a fully-qualified perform links the dispatch channel"
    );
    assert_subset(&outcome);
}
