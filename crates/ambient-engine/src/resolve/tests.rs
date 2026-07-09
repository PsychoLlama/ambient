use super::*;
use crate::ast::{
    AbilityDef, ConstDef, EnumDef, EnumVariant, Expr, ExprKind, FunctionDef, Item, ItemKind,
    MatchArm, Module, Param, Pattern, PatternKind, QualifiedName, Span, StructDef, TypeParam,
};
use crate::types::{NamedType, NominalType, RecordType, Type};

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

/// A bare `Type::Named` head with no args and no stamped identity — the
/// shape every unresolved nominal reference starts as.
fn named(name: &str) -> Type {
    Type::Named(NamedType {
        name: Arc::from(name),
        args: vec![],
        uuid: None,
    })
}

/// `pub fn <name><type_params>(x: <param_ty>) { unit }` — a generic
/// function whose single parameter carries `param_ty` as its annotation.
fn generic_func(name: &str, type_params: &[&str], param_ty: Type) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: type_params
                .iter()
                .map(|p| TypeParam {
                    name: Arc::from(*p),
                    span: Span::default(),
                })
                .collect(),
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

/// The annotated type of function `name`'s first parameter, after resolve.
fn param_ty(module: &Module, name: &str) -> Option<Type> {
    module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) if f.name.as_ref() == name => f.params[0].ty.clone(),
        _ => None,
    })
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

#[test]
fn variant_pattern_resolves_to_two_segment_ident() {
    // A match arm on a variant pattern is stamped with the same
    // `Fqn(module, [Enum, Variant])` a constructor reference gets, so the
    // checker can pick the enum and variant by identity rather than by a
    // collision-prone bare-name reverse lookup.
    let arm = MatchArm {
        pattern: Pattern::variant("V", None),
        guard: None,
        body: Expr::unit(),
    };
    let body = Expr::match_expr(Expr::unit(), vec![arm]);
    let items = vec![
        enum_item("E", uuid::Uuid::from_u128(1)),
        func("f", body, vec![]),
    ];
    let (module, registry, path) = resolve_m(items);
    let resolved = module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) if f.name.as_ref() == "f" => match &f.body.kind {
            ExprKind::Match(_, arms) => match &arms[0].pattern.kind {
                PatternKind::Variant(name, _) => name.resolved.clone(),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        resolved,
        Some(registry.fqn(&path, &[Arc::from("E"), Arc::from("V")]))
    );
}

/// A single-variant enum `<name>` with the given nominal uuid.
fn enum_item(name: &str, uuid: uuid::Uuid) -> Item {
    Item::new(
        ItemKind::Enum(EnumDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![EnumVariant {
                name: Arc::from("V"),
                payload: None,
                span: Span::default(),
            }],
            uuid,
        }),
        Span::default(),
    )
}

/// A struct item with an explicit body, params, and identity.
fn struct_item(
    name: &str,
    type_params: &[&str],
    ty: Type,
    unique_id: Option<uuid::Uuid>,
    is_extern: bool,
) -> Item {
    Item::new(
        ItemKind::Struct(StructDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: type_params
                .iter()
                .map(|p| TypeParam {
                    name: Arc::from(*p),
                    span: Span::default(),
                })
                .collect(),
            ty,
            unique_id,
            is_extern,
        }),
        Span::default(),
    )
}

/// The `Type::Nominal` empty-record body of a unit struct with `uuid`.
fn unit_body(name: &str, uuid: uuid::Uuid) -> Type {
    Type::Nominal(NominalType::new(
        uuid,
        Type::Record(RecordType::new(vec![])),
        Some(name),
    ))
}

#[test]
fn local_enum_type_head_is_stamped() {
    // `enum E { V }` + `fn f(x: E)` — the bare annotation `E` is stamped
    // with the enum's nominal uuid, exactly what a qualified `pkg::m::E`
    // spelling or the checker's `resolve_holes` produces.
    let e_uuid = uuid::Uuid::from_u128(42);
    let (module, _r, _p) = resolve_m(vec![
        enum_item("E", e_uuid),
        generic_func("f", &[], named("E")),
    ]);
    assert_eq!(
        param_ty(&module, "f"),
        Some(Type::Named(NamedType {
            name: Arc::from("E"),
            args: vec![],
            uuid: Some(e_uuid),
        }))
    );
}

#[test]
fn local_non_generic_struct_head_is_substituted() {
    // A plain (non-`unique`) struct annotation expands to its record body.
    let body = Type::record([("x", Type::number())]);
    let (module, _r, _p) = resolve_m(vec![
        struct_item("S", &[], body.clone(), None, false),
        generic_func("f", &[], named("S")),
    ]);
    assert_eq!(param_ty(&module, "f"), Some(body));
}

#[test]
fn local_unique_struct_head_substitutes_carrying_nominal_identity() {
    // A `unique` struct's body is already wrapped in `Type::Nominal`, so
    // substituting it carries the nominal identity to the use site.
    let s_uuid = uuid::Uuid::from_u128(7);
    let body = Type::Nominal(NominalType::new(
        s_uuid,
        Type::record([("v", Type::number())]),
        Some("Id"),
    ));
    let (module, _r, _p) = resolve_m(vec![
        struct_item("Id", &[], body.clone(), Some(s_uuid), false),
        generic_func("f", &[], named("Id")),
    ]);
    assert_eq!(param_ty(&module, "f"), Some(body));
}

#[test]
fn local_opaque_generic_head_is_stamped_with_args() {
    // `extern unique(u) struct List<T>;` — an opaque generic head. The
    // applied form `List<Thing>` keeps its written argument and gains the
    // declaration's uuid; the (undeclared) argument `Thing` stays bare.
    let list_uuid = uuid::Uuid::from_u128(9);
    let list = struct_item(
        "List",
        &["T"],
        unit_body("List", list_uuid),
        Some(list_uuid),
        true,
    );
    let arg = named("Thing");
    let list_ref = Type::Named(NamedType {
        name: Arc::from("List"),
        args: vec![arg.clone()],
        uuid: None,
    });
    let (module, _r, _p) = resolve_m(vec![list, generic_func("f", &[], list_ref)]);
    assert_eq!(
        param_ty(&module, "f"),
        Some(Type::Named(NamedType {
            name: Arc::from("List"),
            args: vec![arg],
            uuid: Some(list_uuid),
        }))
    );
}

#[test]
fn imported_enum_type_head_is_stamped_from_defining_module() {
    use crate::ast::{UseDef, UsePrefix};

    // Module `defs` declares `enum Color { V }`; `main` imports it and
    // annotates `fn f(x: Color)`. The bare `Color` resolves through the
    // import to the defining module and is stamped with its uuid.
    let color_uuid = uuid::Uuid::from_u128(55);
    let defs = Module {
        name: Arc::from("defs"),
        doc: None,
        items: vec![enum_item("Color", color_uuid)],
    };
    let use_color = Item::new(
        ItemKind::Use(UseDef {
            is_public: false,
            prefix: UsePrefix::Pkg,
            path: vec![
                (Arc::from("defs"), Span::default()),
                (Arc::from("Color"), Span::default()),
            ],
            alias: None,
        }),
        Span::default(),
    );
    let mut main = Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![use_color, generic_func("f", &[], named("Color"))],
    };
    let mut registry = ModuleRegistry::new();
    let defs_path = ModulePath::from_str_segments(&["defs"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(&defs_path, Arc::new(defs));
    registry.register(&main_path, Arc::new(main.clone()));
    resolve_module(&mut main, &main_path, &registry);
    assert_eq!(
        param_ty(&main, "f"),
        Some(Type::Named(NamedType {
            name: Arc::from("Color"),
            args: vec![],
            uuid: Some(color_uuid),
        }))
    );
}

#[test]
fn type_param_annotation_stays_bare() {
    // `fn f<T>(x: T)` — the parameter annotation names the function's own
    // type parameter, which has no nominal identity, so resolve leaves it
    // a bare `Named{"T", uuid: None}` (the checker mints a `Type::Param`).
    let (module, _registry, _path) = resolve_m(vec![generic_func("f", &["T"], named("T"))]);
    assert_eq!(param_ty(&module, "f"), Some(named("T")));
}

#[test]
fn type_param_shadowing_a_reserved_name_stays_bare() {
    // `fn f<Number>(x: Number)` — the parameter shadows the primitive
    // `Number`, so the annotation is the type parameter, not the primitive:
    // it must stay bare (Phase 2's stamping keys off `is_type_param` first).
    let (module, _registry, _path) =
        resolve_m(vec![generic_func("f", &["Number"], named("Number"))]);
    assert_eq!(param_ty(&module, "f"), Some(named("Number")));
}
