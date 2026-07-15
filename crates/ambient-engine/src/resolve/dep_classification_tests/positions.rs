//! The original classification table: value positions land in both
//! sets; pure type positions land in `deps` only. See the parent module
//! docs for the full table.

use super::*;

// ─────────────────────────────────────────────────────────────────────────
// VALUE positions: recorded in both `deps` and `link_deps`
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn imported_function_call_is_a_link_dep() {
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![
            use_from_defs("helper"),
            func("g", Expr::name("helper"), vec![]),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn imported_const_reference_is_a_link_dep() {
    let (outcome, defs) = resolve_main(
        vec![const_item("K")],
        vec![use_from_defs("K"), func("g", Expr::name("K"), vec![])],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn imported_enum_variant_construction_is_a_link_dep() {
    let (outcome, defs) = resolve_main(
        vec![enum_item("Shape", "Circle", 0x51)],
        vec![
            use_from_defs("Circle"),
            func("g", Expr::name("Circle"), vec![]),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn imported_unit_struct_value_is_a_link_dep() {
    let (outcome, defs) = resolve_main(
        vec![unit_struct("Marker", 0x52)],
        vec![
            use_from_defs("Marker"),
            func("g", Expr::name("Marker"), vec![]),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

#[test]
fn perform_of_imported_ability_is_a_link_dep() {
    let (outcome, defs) = resolve_main(
        vec![ability_item("Log", 0x53)],
        vec![
            use_from_defs("Log"),
            func(
                "g",
                perform_expr(QualifiedName::simple("Log"), "log"),
                vec![],
            ),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        outcome.link_deps.contains(&defs),
        "a perform links the ability's dispatch channel"
    );
    assert_subset(&outcome);
}

#[test]
fn qualified_function_call_is_a_link_dep() {
    // `pkg::defs::helper` (no `use`) — resolved through `lookup_item`.
    let (outcome, defs) = resolve_main(
        vec![func("helper", Expr::unit(), vec![])],
        vec![func(
            "g",
            name_expr(QualifiedName::qualified(vec!["pkg", "defs"], "helper")),
            vec![],
        )],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(outcome.link_deps.contains(&defs));
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// TYPE positions: recorded in `deps` only, never `link_deps`
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn imported_typed_record_construction_is_check_only() {
    // CHECK-ONLY: the compiler emits *no* link artifact for foreign struct
    // construction — `TypedRecord` lowers to a plain `MakeRecord`, discarding
    // the type name — so a typed record must not manufacture a link-order
    // edge. Recording one would let the candidate-skip in
    // `dispatch_ordering_graph` drop a needed self-orphan dispatch edge. The
    // `use Widget` still records a check-only `deps` edge. See
    // `resolve_type_ref`. (Bare/imported spelling.)
    let (outcome, defs) = resolve_main(
        vec![record_struct("Widget")],
        vec![
            use_from_defs("Widget"),
            func(
                "g",
                typed_record_expr(QualifiedName::simple("Widget")),
                vec![],
            ),
        ],
    );
    assert!(
        outcome.deps.contains(&defs),
        "the checker needs the type's module registered"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "typed-record construction emits no link artifact"
    );
    assert_subset(&outcome);
}

#[test]
fn qualified_typed_record_construction_is_check_only() {
    // The qualified spelling `pkg::defs::Widget { .. }` (no `use`) flows
    // through the shared `resolve_path_ref`/`lookup_item` with `RefPos::Type`,
    // so it agrees with the bare/imported spelling above: a check-only `deps`
    // edge, never a `link_deps` edge.
    let (outcome, defs) = resolve_main(
        vec![record_struct("Widget")],
        vec![func(
            "g",
            typed_record_expr(QualifiedName::qualified(vec!["pkg", "defs"], "Widget")),
            vec![],
        )],
    );
    assert!(
        outcome.deps.contains(&defs),
        "the checker needs the type's module registered"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "a qualified typed-record construction emits no link artifact either"
    );
    assert_subset(&outcome);
}

#[test]
fn qualified_type_annotation_is_check_only() {
    // `fn f(x: pkg::defs::Color)` — pure type consumption, no `use`.
    let (outcome, defs) = resolve_main(
        vec![enum_item("Color", "Red", 0x54)],
        vec![func_with_param("f", named("pkg::defs::Color"))],
    );
    assert!(
        outcome.deps.contains(&defs),
        "the checker needs the type's module registered"
    );
    assert!(
        !outcome.link_deps.contains(&defs),
        "a type annotation emits no link artifact"
    );
    assert_subset(&outcome);
}

#[test]
fn impl_of_imported_trait_for_imported_type_is_check_only() {
    // `use pkg::defs::{Widget, Describe};
    //  impl Describe for Widget { fn describe(self): Number { 1 } }`
    // The trait header records nothing (traits never add ordering edges);
    // the impl target is a bare type head (nothing); only the two `use`
    // imports record a check-only dep. No link edge.
    let describe = describe_method();
    let imp = Item::new(
        ItemKind::Impl(ImplDef {
            type_params: vec![],
            trait_name: Some(crate::ast::TraitRef {
                name: QualifiedName::simple("Describe"),
                args: vec![],
            }),
            for_type: named("Widget"),
            assoc_types: vec![],
            methods: vec![describe],
            span: Span::default(),
        }),
        Span::default(),
    );
    let (outcome, defs) = resolve_main(
        vec![trait_item("Describe", 0x55), record_struct("Widget")],
        vec![use_from_defs("Widget"), use_from_defs("Describe"), imp],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        !outcome.link_deps.contains(&defs),
        "implementing an imported trait for an imported type is not a link edge"
    );
    assert_subset(&outcome);
}

#[test]
fn associated_call_records_no_link_dep() {
    // `use pkg::defs::Money; Money::default()`. The resolve pass leaves the
    // associated `Type::method` call unresolved (the checker dispatches it),
    // so it records no resolve-pass dep at all: the impl-module link edge is
    // a *dispatch* edge handled by `dispatch_scope`/`reachability`, not here.
    // Only the `use Money` records a check-only dep.
    let (outcome, defs) = resolve_main(
        vec![record_struct("Money")],
        vec![
            use_from_defs("Money"),
            func(
                "g",
                name_expr(QualifiedName::qualified(vec!["Money"], "default")),
                vec![],
            ),
        ],
    );
    assert!(outcome.deps.contains(&defs));
    assert!(
        !outcome.link_deps.contains(&defs),
        "an associated call records no resolve-pass link edge"
    );
    assert_subset(&outcome);
}

// ─────────────────────────────────────────────────────────────────────────
// Mixed fixture: subset holds with both kinds present
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn mixed_value_and_type_references_keep_link_deps_a_strict_subset() {
    // `value_defs` is called (link); `type_defs` is only annotated (check).
    let mut registry = ModuleRegistry::new();
    let value_path = ModulePath::from_str_segments(&["value_defs"]).unwrap();
    let type_path = ModulePath::from_str_segments(&["type_defs"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &value_path,
        Arc::new(crate::ast::Module {
            name: Arc::from("value_defs"),
            doc: None,
            items: vec![func("helper", Expr::unit(), vec![])],
        }),
    );
    registry.register(
        &type_path,
        Arc::new(crate::ast::Module {
            name: Arc::from("type_defs"),
            doc: None,
            items: vec![enum_item("Color", "Red", 0x56)],
        }),
    );
    let mut main = crate::ast::Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![
            func(
                "g",
                name_expr(QualifiedName::qualified(
                    vec!["pkg", "value_defs"],
                    "helper",
                )),
                vec![],
            ),
            func_with_param("f", named("pkg::type_defs::Color")),
        ],
    };
    registry.register(&main_path, Arc::new(main.clone()));
    let outcome = resolve_module(&mut main, &main_path, &registry);

    let value_id = registry.module_id(&value_path);
    let type_id = registry.module_id(&type_path);
    assert!(outcome.deps.contains(&value_id));
    assert!(outcome.deps.contains(&type_id));
    assert!(outcome.link_deps.contains(&value_id));
    assert!(
        !outcome.link_deps.contains(&type_id),
        "the type-only edge must stay out of link_deps"
    );
    assert!(outcome.link_deps.is_subset(&outcome.deps));
    assert!(
        outcome.link_deps.len() < outcome.deps.len(),
        "link_deps is a strict subset when a type-only edge is present"
    );
}
