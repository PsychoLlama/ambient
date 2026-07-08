use super::*;
use crate::ast::{
    AbilityDef, ConstDef, EnumDef, EnumVariant, Expr, FunctionDef, Item, ItemKind, Module,
    QualifiedName, Span,
};
use crate::types::Type;

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

/// Resolve module `m` (single-package registry) and return it.
fn resolve_m(items: Vec<Item>) -> (Module, ModuleRegistry, ModulePath) {
    let mut module = Module {
        name: Arc::from("m"),
        doc: None,
        items,
    };
    let mut registry = ModuleRegistry::new();
    let path = ModulePath::from_str_segments(&["m"]).unwrap();
    registry.register(&path, Arc::new(module.clone()));
    resolve_module(&mut module, &path, &registry);
    (module, registry, path)
}

/// The `resolved` of the body `Name` of function `name`.
fn body_resolved(module: &Module, name: &str) -> Option<Fqn> {
    module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) if f.name.as_ref() == name => match &f.body.kind {
            crate::ast::ExprKind::Name(n) => n.resolved.clone(),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn same_module_function_reference_resolves_to_its_fqn() {
    let items = vec![
        func("a", Expr::unit(), vec![]),
        func("ref_a", Expr::name("a"), vec![]),
    ];
    let (module, registry, path) = resolve_m(items);
    assert_eq!(
        body_resolved(&module, "ref_a"),
        Some(registry.fqn(&path, &[Arc::from("a")]))
    );
}

#[test]
fn same_module_const_reference_resolves_to_its_fqn() {
    let konst = Item::new(
        ItemKind::Const(ConstDef {
            id: 0,
            name: Arc::from("K"),
            name_span: Span::default(),
            is_public: true,
            ty: Some(Type::number()),
            value: Expr::number(1.0),
        }),
        Span::default(),
    );
    let items = vec![konst, func("ref_k", Expr::name("K"), vec![])];
    let (module, registry, path) = resolve_m(items);
    assert_eq!(
        body_resolved(&module, "ref_k"),
        Some(registry.fqn(&path, &[Arc::from("K")]))
    );
}

#[test]
fn same_module_enum_variant_resolves_to_two_segment_ident() {
    let enum_def = Item::new(
        ItemKind::Enum(EnumDef {
            name: Arc::from("E"),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![EnumVariant {
                name: Arc::from("V"),
                payload: None,
                span: Span::default(),
            }],
            uuid: uuid::Uuid::from_u128(1),
        }),
        Span::default(),
    );
    let items = vec![enum_def, func("ref_v", Expr::name("V"), vec![])];
    let (module, registry, path) = resolve_m(items);
    // The variant's ident is `[Enum, Variant]`, not the bare `[V]`.
    assert_eq!(
        body_resolved(&module, "ref_v"),
        Some(registry.fqn(&path, &[Arc::from("E"), Arc::from("V")]))
    );
}

#[test]
fn imported_enum_variant_resolves_to_two_segment_ident() {
    use crate::ast::{UseDef, UsePrefix};

    // Module `shapes` declares `enum Shape { Circle }`.
    let shapes = Module {
        name: Arc::from("shapes"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Enum(EnumDef {
                name: Arc::from("Shape"),
                name_span: Span::default(),
                is_public: true,
                type_params: vec![],
                variants: vec![EnumVariant {
                    name: Arc::from("Circle"),
                    payload: None,
                    span: Span::default(),
                }],
                uuid: uuid::Uuid::from_u128(2),
            }),
            Span::default(),
        )],
    };
    // Module `main` imports the variant by name and references it.
    let use_item = Item::new(
        ItemKind::Use(UseDef {
            is_public: false,
            prefix: UsePrefix::Pkg,
            path: vec![
                (Arc::from("shapes"), Span::default()),
                (Arc::from("Circle"), Span::default()),
            ],
            alias: None,
        }),
        Span::default(),
    );
    let mut main = Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![use_item, func("ref_circle", Expr::name("Circle"), vec![])],
    };

    let mut registry = ModuleRegistry::new();
    let shapes_path = ModulePath::from_str_segments(&["shapes"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(&shapes_path, Arc::new(shapes));
    registry.register(&main_path, Arc::new(main.clone()));
    resolve_module(&mut main, &main_path, &registry);

    // The imported variant lands on `shapes::Shape::Circle`, not a bare
    // `Circle` (corner #2: imported variants now resolve).
    assert_eq!(
        body_resolved(&main, "ref_circle"),
        Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
    );
}

/// Build a `shapes` module (`enum Shape { Circle, Dot }`) plus a `main`
/// that references a variant through `callee`, resolve `main`, and return
/// it with the registry and shapes path.
fn shapes_and_ref(callee: QualifiedName) -> (Module, ModuleRegistry, ModulePath) {
    let shapes = Module {
        name: Arc::from("shapes"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Enum(EnumDef {
                name: Arc::from("Shape"),
                name_span: Span::default(),
                is_public: true,
                type_params: vec![],
                variants: vec![
                    EnumVariant {
                        name: Arc::from("Circle"),
                        payload: None,
                        span: Span::default(),
                    },
                    EnumVariant {
                        name: Arc::from("Dot"),
                        payload: None,
                        span: Span::default(),
                    },
                ],
                uuid: uuid::Uuid::from_u128(3),
            }),
            Span::default(),
        )],
    };
    let body = Expr {
        kind: crate::ast::ExprKind::Name(callee),
        span: Span::default(),
        ty: None,
    };
    let mut main = Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![func("ref_circle", body, vec![])],
    };
    let mut registry = ModuleRegistry::new();
    let shapes_path = ModulePath::from_str_segments(&["shapes"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(&shapes_path, Arc::new(shapes));
    registry.register(&main_path, Arc::new(main.clone()));
    resolve_module(&mut main, &main_path, &registry);
    (main, registry, shapes_path)
}

#[test]
fn foreign_variant_qualified_by_module_resolves_to_two_segment_ident() {
    // `pkg::shapes::Circle` — the variant is a direct export of module
    // `shapes` — lands on the two-segment `shapes::Shape::Circle`.
    let (main, registry, shapes_path) =
        shapes_and_ref(QualifiedName::qualified(vec!["pkg", "shapes"], "Circle"));
    assert_eq!(
        body_resolved(&main, "ref_circle"),
        Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
    );
}

#[test]
fn foreign_variant_qualified_by_enum_resolves_to_two_segment_ident() {
    // `pkg::shapes::Shape::Circle` — the explicit-enum spelling, where the
    // last path segment names the enum, not a module — lands on the same
    // two-segment ident (via the `resolve_explicit_enum_variant` fallback).
    let (main, registry, shapes_path) = shapes_and_ref(QualifiedName::qualified(
        vec!["pkg", "shapes", "Shape"],
        "Circle",
    ));
    assert_eq!(
        body_resolved(&main, "ref_circle"),
        Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
    );
}

#[test]
fn prelude_name_creates_a_dep_only_when_referenced() {
    use crate::ast::{Item, UseDef, UsePrefix};

    // `donor` defines `gift`; `pre` re-exports it and is the prelude.
    let mut registry = ModuleRegistry::new();
    let donor = ModulePath::from_str_segments(&["donor"]).unwrap();
    registry.register(
        &donor,
        Arc::new(Module {
            name: Arc::from("donor"),
            doc: None,
            items: vec![func("gift", Expr::unit(), vec![])],
        }),
    );
    let pre = ModulePath::from_str_segments(&["pre"]).unwrap();
    registry.register(
        &pre,
        Arc::new(Module {
            name: Arc::from("pre"),
            doc: None,
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: true,
                    prefix: UsePrefix::Pkg,
                    path: vec![
                        (Arc::from("donor"), Span::default()),
                        (Arc::from("gift"), Span::default()),
                    ],
                    alias: None,
                }),
                Span::default(),
            )],
        }),
    );
    registry.set_prelude(pre);

    let donor_id = registry.module_id(&donor);

    // A module that never references `gift` gains no dependency on
    // `donor`: the prelude tier is not folded into the dep closure.
    let mut quiet = Module {
        name: Arc::from("quiet"),
        doc: None,
        items: vec![func("noop", Expr::unit(), vec![])],
    };
    let quiet_path = ModulePath::from_str_segments(&["quiet"]).unwrap();
    registry.register(&quiet_path, Arc::new(quiet.clone()));
    let outcome = resolve_module(&mut quiet, &quiet_path, &registry);
    assert!(
        !outcome.deps.contains(&donor_id),
        "an unreferenced prelude name must not create a dep edge"
    );

    // A module that *does* reference `gift` resolves it to donor's `Fqn`
    // and gains the dependency.
    let mut user = Module {
        name: Arc::from("user"),
        doc: None,
        items: vec![func("use_gift", Expr::name("gift"), vec![])],
    };
    let user_path = ModulePath::from_str_segments(&["user"]).unwrap();
    registry.register(&user_path, Arc::new(user.clone()));
    let outcome = resolve_module(&mut user, &user_path, &registry);
    assert_eq!(
        body_resolved(&user, "use_gift"),
        Some(registry.fqn(&donor, &[Arc::from("gift")])),
        "a referenced prelude name resolves to its origin `Fqn`"
    );
    assert!(
        outcome.deps.contains(&donor_id),
        "a referenced prelude name creates the dep edge"
    );
}

#[test]
fn same_module_ability_reference_resolves_to_its_fqn() {
    let ability = Item::new(
        ItemKind::Ability(AbilityDef {
            name: Arc::from("A"),
            name_span: Span::default(),
            is_public: true,
            dependencies: vec![],
            methods: vec![],
            resolved_id: None,
        }),
        Span::default(),
    );
    let items = vec![
        ability,
        func("with_a", Expr::unit(), vec![QualifiedName::simple("A")]),
    ];
    let (module, registry, path) = resolve_m(items);
    let resolved = module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) if f.name.as_ref() == "with_a" => {
            f.abilities.first().and_then(|q| q.resolved.clone())
        }
        _ => None,
    });
    assert_eq!(resolved, Some(registry.fqn(&path, &[Arc::from("A")])));
}
