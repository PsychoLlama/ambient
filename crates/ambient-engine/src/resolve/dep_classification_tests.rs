//! Phase-2 dependency-classification tests.
//!
//! The resolve pass now records two dependency sets on [`ResolveOutcome`]:
//! the full `deps` (every reference, value *and* type, plus `use` imports)
//! and its link-order subset `link_deps` (only value/symbol positions the
//! compiler emits a link-time artifact for). These tests pin the boundary:
//!
//! - value positions (calls, consts, variant/unit-struct construction,
//!   ability performs) land in **both** sets;
//! - pure type positions (qualified type annotations, impl targets/headers,
//!   the `use` statement itself, associated `Type::method` calls, and
//!   typed-record construction — the compiler discards the type name) land in
//!   `deps` **only**;
//! - `link_deps ⊆ deps` on every fixture.
//!
//! `link_deps`'s sole consumer is the compile-ordering graph
//! (`dispatch_ordering_graph`); these guard the boundary it keys off.

use std::sync::Arc;

use super::{ResolveOutcome, resolve_module};
use crate::ast::{
    AbilityCall, AbilityDef, ConstDef, EnumDef, EnumVariant, Expr, ExprKind, FunctionDef, ImplDef,
    Item, ItemKind, Param, QualifiedName, Span, StructDef, TraitDef, UseDef, UsePrefix,
};
use crate::fqn::ModuleId;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::types::{NamedType, NominalType, RecordType, Type};

// ─────────────────────────────────────────────────────────────────────────
// Fixture builders
// ─────────────────────────────────────────────────────────────────────────

fn func(name: &str, body: Expr, abilities: Vec<QualifiedName>) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities,
            body,
        }),
        Span::default(),
    )
}

/// A public function with one annotated parameter (`fn f(x: <ty>) { unit }`) —
/// used to exercise type-position annotations.
fn func_with_param(name: &str, param_ty: Type) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            params: vec![Param {
                id: 0,
                name: Arc::from("x"),
                ty: Some(param_ty),
                span: Span::default(),
            }],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        }),
        Span::default(),
    )
}

fn const_item(name: &str) -> Item {
    Item::new(
        ItemKind::Const(ConstDef {
            id: 0,
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            ty: Some(Type::number()),
            value: Expr::number(1.0),
        }),
        Span::default(),
    )
}

fn enum_item(name: &str, variant: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Enum(EnumDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![EnumVariant {
                name: Arc::from(variant),
                payload: None,
                span: Span::default(),
            }],
            uuid: uuid::Uuid::from_u128(uuid),
        }),
        Span::default(),
    )
}

/// A `unique(uuid) struct <name>;` — a unit struct whose bare name is a value.
fn unit_struct(name: &str, uuid: u128) -> Item {
    let id = uuid::Uuid::from_u128(uuid);
    Item::new(
        ItemKind::Struct(StructDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            ty: Type::Nominal(NominalType::new(
                id,
                Type::Record(RecordType::new(vec![])),
                Some(name),
            )),
            unique_id: Some(id),
            is_extern: false,
        }),
        Span::default(),
    )
}

/// A plain fielded struct (`struct <name> { v: Number }`) — a type that is
/// constructed with a typed record, not by its bare name.
fn record_struct(name: &str) -> Item {
    Item::new(
        ItemKind::Struct(StructDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            ty: Type::record([("v", Type::number())]),
            unique_id: None,
            is_extern: false,
        }),
        Span::default(),
    )
}

fn ability_item(name: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Ability(AbilityDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            dependencies: vec![],
            methods: vec![],
            uuid: uuid::Uuid::from_u128(uuid),
            resolved_id: None,
        }),
        Span::default(),
    )
}

fn trait_item(name: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Trait(TraitDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            uuid: uuid::Uuid::from_u128(uuid),
            type_params: vec![],
            supertraits: vec![],
            methods: vec![],
        }),
        Span::default(),
    )
}

/// `use pkg::defs::<name>;`
fn use_from_defs(name: &str) -> Item {
    Item::new(
        ItemKind::Use(UseDef {
            is_public: false,
            prefix: UsePrefix::Pkg,
            path: vec![
                (Arc::from("defs"), Span::default()),
                (Arc::from(name), Span::default()),
            ],
            alias: None,
        }),
        Span::default(),
    )
}

fn named(name: &str) -> Type {
    Type::Named(NamedType {
        name: Arc::from(name),
        args: vec![],
        uuid: None,
    })
}

fn name_expr(qn: QualifiedName) -> Expr {
    Expr {
        kind: ExprKind::Name(qn),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

fn perform_expr(ability: &str, method: &str) -> Expr {
    Expr {
        kind: ExprKind::Perform(AbilityCall {
            ability: QualifiedName::simple(ability),
            method: Arc::from(method),
            args: vec![],
            fingerprints: None,
            span: Span::default(),
        }),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

fn typed_record_expr(type_name: QualifiedName) -> Expr {
    Expr {
        kind: ExprKind::TypedRecord {
            type_name,
            fields: vec![],
        },
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

/// Register a `defs` module and a `main` module referencing it, resolve
/// `main`, and return its outcome alongside `defs`'s [`ModuleId`].
fn resolve_main(defs_items: Vec<Item>, main_items: Vec<Item>) -> (ResolveOutcome, ModuleId) {
    let mut registry = ModuleRegistry::new();
    let defs_path = ModulePath::from_str_segments(&["defs"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    let mut main = crate::ast::Module {
        name: Arc::from("main"),
        doc: None,
        items: main_items,
    };
    registry.register(
        &defs_path,
        Arc::new(crate::ast::Module {
            name: Arc::from("defs"),
            doc: None,
            items: defs_items,
        }),
    );
    registry.register(&main_path, Arc::new(main.clone()));
    let outcome = resolve_module(&mut main, &main_path, &registry);
    let defs_id = registry.module_id(&defs_path);
    (outcome, defs_id)
}

/// Assert the superset invariant on every outcome.
fn assert_subset(outcome: &ResolveOutcome) {
    assert!(
        outcome.link_deps.is_subset(&outcome.deps),
        "link_deps ({:?}) must be a subset of deps ({:?})",
        outcome.link_deps,
        outcome.deps,
    );
}

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
            func("g", perform_expr("Log", "log"), vec![]),
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
            trait_name: Some(QualifiedName::simple("Describe")),
            for_type: named("Widget"),
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

/// `fn describe(self): Number { 1 }` as an impl method.
fn describe_method() -> crate::ast::ImplMethod {
    crate::ast::ImplMethod {
        name: Arc::from("describe"),
        name_span: Span::default(),
        type_params: vec![],
        has_self: true,
        self_id: 0,
        params: vec![],
        ret_ty: Some(Type::number()),
        abilities: vec![],
        body: Expr::number(1.0),
        span: Span::default(),
        resolved_symbol: None,
    }
}
