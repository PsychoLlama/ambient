use super::*;
use crate::ast::{ConstDef, Expr, FunctionDef, Item, Span};

fn make_function(name: &str, is_public: bool) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        }),
        Span::default(),
    )
}

fn make_const(name: &str, value: f64, is_public: bool) -> Item {
    use crate::types::Type;
    Item::new(
        ItemKind::Const(ConstDef {
            id: 0,
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            ty: Some(Type::number()),
            value: Expr::number(value),
        }),
        Span::default(),
    )
}

fn make_enum(name: &str, variants: &[&str], is_public: bool) -> Item {
    use crate::ast::{EnumDef, EnumVariant};
    Item::new(
        ItemKind::Enum(EnumDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            type_params: vec![],
            variants: variants
                .iter()
                .map(|v| EnumVariant {
                    name: Arc::from(*v),
                    payload: None,
                    span: Span::default(),
                })
                .collect(),
            uuid: uuid::Uuid::nil(),
        }),
        Span::default(),
    )
}

fn make_trait(name: &str, is_public: bool) -> Item {
    use crate::ast::TraitDef;
    Item::new(
        ItemKind::Trait(TraitDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            type_params: vec![],
            supertraits: vec![],
            methods: vec![],
        }),
        Span::default(),
    )
}

fn make_ability(name: &str, is_public: bool) -> Item {
    use crate::ast::AbilityDef;
    Item::new(
        ItemKind::Ability(AbilityDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            dependencies: vec![],
            methods: vec![],
            resolved_id: None,
        }),
        Span::default(),
    )
}

#[test]
fn test_register_and_lookup() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("utils"),
        doc: None,
        items: vec![
            make_function("helper", true),
            make_function("internal", false),
        ],
    });

    let path = ModulePath::from_str_segments(&["utils"]).unwrap();
    registry.register(&path, module);

    // Public function should be found
    let result = registry.lookup_symbol(&path, "helper");
    assert!(result.is_ok());
    let (export, origin) = result.unwrap();
    assert_eq!(export.kind, ExportKind::Function);
    assert_eq!(origin, path);

    // Private function should error
    let result = registry.lookup_symbol(&path, "internal");
    assert!(matches!(result, Err(RegistryError::NotPublic { .. })));
}

#[test]
fn test_contains() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![],
    });

    let path = ModulePath::from_str_segments(&["test"]).unwrap();
    assert!(!registry.contains(&path));

    registry.register(&path, module);
    assert!(registry.contains(&path));
}

#[test]
fn test_module_not_found() {
    let registry = ModuleRegistry::new();
    let path = ModulePath::from_str_segments(&["nonexistent"]).unwrap();

    let result = registry.lookup_symbol(&path, "anything");
    assert!(matches!(result, Err(RegistryError::ModuleNotFound(_))));
}

#[test]
fn test_symbol_not_found() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![make_function("foo", true)],
    });

    let path = ModulePath::from_str_segments(&["test"]).unwrap();
    registry.register(&path, module);

    let result = registry.lookup_symbol(&path, "bar");
    assert!(matches!(result, Err(RegistryError::SymbolNotFound { .. })));
}

#[test]
#[allow(clippy::approx_constant)] // 3.14159 is arbitrary const-value test data, not π
fn test_get_public_exports() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            make_function("public1", true),
            make_function("public2", true),
            make_function("private", false),
            make_const("PI", 3.14159, true),
        ],
    });

    let path = ModulePath::from_str_segments(&["test"]).unwrap();
    registry.register(&path, module);

    let exports = registry.get_public_exports(&path);
    assert_eq!(exports.len(), 3); // 2 public functions + 1 const
}

#[test]
fn private_items_are_not_importable() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            make_const("SECRET", 42.0, false),
            make_enum("Hidden", &["A", "B"], false),
            make_trait("Sealed", false),
            make_ability("Internal", false),
        ],
    });

    let path = ModulePath::from_str_segments(&["test"]).unwrap();
    registry.register(&path, module);

    for symbol in ["SECRET", "Hidden", "A", "B", "Sealed", "Internal"] {
        let result = registry.lookup_symbol(&path, symbol);
        assert!(
            matches!(result, Err(RegistryError::NotPublic { .. })),
            "expected NotPublic for `{symbol}`, got {result:?}"
        );
    }

    assert!(registry.get_public_exports(&path).is_empty());
}

#[test]
fn public_items_are_importable() {
    let mut registry = ModuleRegistry::new();

    let module = Arc::new(Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            make_const("ANSWER", 42.0, true),
            make_enum("Visible", &["Yes"], true),
            make_trait("Open", true),
            make_ability("Exposed", true),
        ],
    });

    let path = ModulePath::from_str_segments(&["test"]).unwrap();
    registry.register(&path, module);

    let cases = [
        ("ANSWER", ExportKind::Const),
        ("Visible", ExportKind::Enum),
        ("Yes", ExportKind::EnumVariant),
        ("Open", ExportKind::Trait),
        ("Exposed", ExportKind::Ability),
    ];
    for (symbol, kind) in cases {
        let (export, _) = registry
            .lookup_symbol(&path, symbol)
            .unwrap_or_else(|e| panic!("expected `{symbol}` to be public, got {e:?}"));
        assert_eq!(export.kind, kind);
    }

    // Enum + variant + const + trait + ability
    assert_eq!(registry.get_public_exports(&path).len(), 5);
}

#[test]
fn test_resolve_use_path_pkg() {
    let registry = ModuleRegistry::new();
    let from = ModulePath::from_str_segments(&["main"]).unwrap();
    let path = vec![Arc::from("utils"), Arc::from("format")];

    let resolved = registry.resolve_use_path(&from, &UsePrefix::Pkg, &path);
    assert!(resolved.is_ok());
    assert_eq!(resolved.unwrap().to_string(), "utils::format");
}

#[test]
fn test_resolve_use_path_self() {
    let registry = ModuleRegistry::new();
    let from = ModulePath::from_str_segments(&["utils", "main"]).unwrap();
    let path = vec![Arc::from("sibling")];

    let resolved = registry.resolve_use_path(&from, &UsePrefix::Self_, &path);
    assert!(resolved.is_ok());
    assert_eq!(resolved.unwrap().to_string(), "utils::sibling");
}

#[test]
fn test_resolve_use_path_super() {
    let registry = ModuleRegistry::new();
    let from = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
    let path = vec![Arc::from("other")];

    let resolved = registry.resolve_use_path(&from, &UsePrefix::Super(1), &path);
    assert!(resolved.is_ok());
    assert_eq!(resolved.unwrap().to_string(), "a::other");
}

#[test]
fn test_resolve_use_path_core() {
    let registry = ModuleRegistry::new();
    let from = ModulePath::from_str_segments(&["main"]).unwrap();
    let path = vec![Arc::from("List")];

    let resolved = registry
        .resolve_use_path(&from, &UsePrefix::Core, &path)
        .expect("core resolves under the reserved root");
    assert_eq!(resolved.to_string(), "core::List");
}

#[test]
fn test_resolve_imports_items() {
    use crate::ast::{Item, UseDef};

    let mut registry = ModuleRegistry::new();

    // Register the utils module with a helper function
    let utils_module = Arc::new(Module {
        name: Arc::from("utils"),
        doc: None,
        items: vec![make_function("helper", true)],
    });
    let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
    registry.register(&utils_path, utils_module);

    // Register main module with a use statement
    let main_module = Arc::new(Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Use(UseDef {
                is_public: false,
                prefix: UsePrefix::Pkg,
                path: vec![
                    (Arc::from("utils"), Span::default()),
                    (Arc::from("helper"), Span::default()),
                ],
                alias: None,
            }),
            Span::default(),
        )],
    });
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(&main_path, main_module);

    // Resolve imports for main module
    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.errors.is_empty());
    match resolved.imports["helper"].as_slice() {
        [
            ResolvedImport::Symbol {
                from_module,
                export_kind,
                ..
            },
        ] => {
            assert_eq!(from_module.to_string(), "utils");
            assert_eq!(*export_kind, ExportKind::Function);
        }
        other => panic!("Expected a single symbol import, got {other:?}"),
    }
}

/// A `pub use pkg::donor::gift;` item, for building a prelude module.
fn pub_use(segments: &[&str]) -> crate::ast::Item {
    use crate::ast::{Item, UseDef};
    Item::new(
        ItemKind::Use(UseDef {
            is_public: true,
            prefix: UsePrefix::Pkg,
            path: segments
                .iter()
                .map(|s| (Arc::from(*s), Span::default()))
                .collect(),
            alias: None,
        }),
        Span::default(),
    )
}

#[test]
fn prelude_binds_at_lowest_precedence_and_imports_shadow_it() {
    // `donor` defines `gift`; `pre` re-exports it; `pre` is the prelude.
    let mut registry = ModuleRegistry::new();
    let donor = ModulePath::from_str_segments(&["donor"]).unwrap();
    registry.register(
        &donor,
        Arc::new(Module {
            name: Arc::from("donor"),
            doc: None,
            items: vec![make_function("gift", true)],
        }),
    );
    let other = ModulePath::from_str_segments(&["other"]).unwrap();
    registry.register(
        &other,
        Arc::new(Module {
            name: Arc::from("other"),
            doc: None,
            items: vec![make_function("gift", true)],
        }),
    );
    let pre = ModulePath::from_str_segments(&["pre"]).unwrap();
    registry.register(
        &pre,
        Arc::new(Module {
            name: Arc::from("pre"),
            doc: None,
            items: vec![pub_use(&["donor", "gift"])],
        }),
    );
    registry.set_prelude(pre.clone());

    // A consumer with no `use` sees `gift` only through the prelude tier —
    // never in `items` (which the dep-closure loop turns into edges).
    let consumer = ModulePath::from_str_segments(&["consumer"]).unwrap();
    registry.register(
        &consumer,
        Arc::new(Module {
            name: Arc::from("consumer"),
            doc: None,
            items: vec![],
        }),
    );
    let scope = registry.build_module_scope(&consumer);
    assert!(
        !scope.items.contains_key("gift"),
        "prelude names must not live in `items`"
    );
    let bound = scope
        .item("gift", Namespace::Value)
        .expect("prelude `gift` resolves");
    assert_eq!(bound.module, donor);

    // An explicit `use pkg::other::gift;` shadows the prelude at the same
    // name: `items` wins over `prelude_items`.
    let shadower = ModulePath::from_str_segments(&["shadower"]).unwrap();
    registry.register(
        &shadower,
        Arc::new(Module {
            name: Arc::from("shadower"),
            doc: None,
            items: vec![{
                use crate::ast::{Item, UseDef};
                Item::new(
                    ItemKind::Use(UseDef {
                        is_public: false,
                        prefix: UsePrefix::Pkg,
                        path: vec![
                            (Arc::from("other"), Span::default()),
                            (Arc::from("gift"), Span::default()),
                        ],
                        alias: None,
                    }),
                    Span::default(),
                )
            }],
        }),
    );
    let scope = registry.build_module_scope(&shadower);
    let bound = scope
        .item("gift", Namespace::Value)
        .expect("explicit `gift` resolves");
    assert_eq!(bound.module, other, "an explicit `use` shadows the prelude");
}

#[test]
fn test_resolve_imports_module() {
    use crate::ast::{Item, UseDef};

    let mut registry = ModuleRegistry::new();

    // Register the utils module
    let utils_module = Arc::new(Module {
        name: Arc::from("utils"),
        doc: None,
        items: vec![make_function("helper", true)],
    });
    let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
    registry.register(&utils_path, utils_module);

    // Register main module with a module import
    let main_module = Arc::new(Module {
        name: Arc::from("main"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Use(UseDef {
                is_public: false,
                prefix: UsePrefix::Pkg,
                path: vec![(Arc::from("utils"), Span::default())],
                alias: None,
            }),
            Span::default(),
        )],
    });
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(&main_path, main_module);

    // Resolve imports for main module
    let resolved = registry.resolve_imports(&main_path).unwrap();
    // "utils" should be imported as a module reference
    assert!(resolved.errors.is_empty());
    assert!(matches!(
        resolved.imports["utils"].as_slice(),
        [ResolvedImport::Module(_)]
    ));
}

fn use_module(prefix: UsePrefix, path: &[&str], is_public: bool) -> Item {
    use crate::ast::UseDef;
    Item::new(
        ItemKind::Use(UseDef {
            is_public,
            prefix,
            path: path
                .iter()
                .map(|s| (Arc::from(*s), Span::default()))
                .collect(),
            alias: None,
        }),
        Span::default(),
    )
}

/// The non-brace form imports an item just like the brace form:
/// `use pkg::utils::helper` binds the symbol `helper` when `utils`
/// exports it, rather than demanding a submodule named `helper`.
#[test]
fn non_brace_path_imports_a_symbol() {
    let mut registry = ModuleRegistry::new();

    let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
    registry.register(
        &utils_path,
        Arc::new(Module {
            name: Arc::from("utils"),
            doc: None,
            items: vec![make_function("helper", true)],
        }),
    );

    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &main_path,
        Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![use_module(UsePrefix::Pkg, &["utils", "helper"], false)],
        }),
    );

    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
    assert!(
        matches!(
            resolved.imports["helper"].as_slice(),
            [ResolvedImport::Symbol { .. }]
        ),
        "got {:?}",
        resolved.imports.get("helper")
    );
}

/// Symmetry the other way: the brace form imports a submodule just like
/// the non-brace form. `use pkg::a::{b}` binds submodule `a::b`.
#[test]
fn brace_form_imports_a_submodule() {
    let mut registry = ModuleRegistry::new();

    // Register submodule `a.b` and its parent `a`.
    for path in [["a"].as_slice(), ["a", "b"].as_slice()] {
        let module_path = ModulePath::from_str_segments(path).unwrap();
        registry.register(
            &module_path,
            Arc::new(Module {
                name: Arc::from(*path.last().unwrap()),
                doc: None,
                items: vec![],
            }),
        );
    }

    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &main_path,
        Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: use_items(UsePrefix::Pkg, &["a"], &["b"], false),
        }),
    );

    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
    assert!(matches!(
        resolved.imports["b"].as_slice(),
        [ResolvedImport::Module(_)]
    ));
}

/// When a name is both a submodule of the parent and a symbol it
/// exports, `use` binds both — the use site disambiguates by position.
#[test]
fn name_that_is_both_submodule_and_symbol_binds_both() {
    let mut registry = ModuleRegistry::new();

    // `a` exports a symbol `b`, and `a.b` is also a submodule.
    registry.register(
        &ModulePath::from_str_segments(&["a"]).unwrap(),
        Arc::new(Module {
            name: Arc::from("a"),
            doc: None,
            items: vec![make_function("b", true)],
        }),
    );
    registry.register(
        &ModulePath::from_str_segments(&["a", "b"]).unwrap(),
        Arc::new(Module {
            name: Arc::from("b"),
            doc: None,
            items: vec![],
        }),
    );

    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &main_path,
        Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![use_module(UsePrefix::Pkg, &["a", "b"], false)],
        }),
    );

    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
    let bindings = &resolved.imports["b"];
    assert_eq!(
        bindings.len(),
        2,
        "expected both bindings, got {bindings:?}"
    );
    assert!(
        bindings
            .iter()
            .any(|b| matches!(b, ResolvedImport::Module(_)))
    );
    assert!(
        bindings
            .iter()
            .any(|b| matches!(b, ResolvedImport::Symbol { .. }))
    );
}

/// A non-brace `pub use pkg::origin::helper` re-exports the item just
/// like the braced form — braces are grouping on the re-export side too.
#[test]
fn non_brace_re_export_resolves_to_origin() {
    let mut registry = ModuleRegistry::new();

    let origin_path = ModulePath::from_str_segments(&["origin"]).unwrap();
    registry.register(
        &origin_path,
        Arc::new(Module {
            name: Arc::from("origin"),
            doc: None,
            items: vec![make_function("helper", true)],
        }),
    );

    // facade re-exports `helper` without braces.
    let facade_path = ModulePath::from_str_segments(&["facade"]).unwrap();
    registry.register(
        &facade_path,
        Arc::new(Module {
            name: Arc::from("facade"),
            doc: None,
            items: vec![use_module(UsePrefix::Pkg, &["origin", "helper"], true)],
        }),
    );

    let (_, origin) = registry
        .lookup_symbol(&facade_path, "helper")
        .expect("non-brace re-export should resolve");
    assert_eq!(origin, origin_path);
}

/// The old braced form flattens to one `UseDef` per item at lowering;
/// tests build the flattened items directly.
fn use_items(prefix: UsePrefix, path: &[&str], items: &[&str], is_public: bool) -> Vec<Item> {
    items
        .iter()
        .map(|item| {
            let mut full: Vec<&str> = path.to_vec();
            full.push(item);
            use_module(prefix, &full, is_public)
        })
        .collect()
}

/// `pub use` chains resolve to the module that defines the symbol,
/// not the module that re-exports it — that is where compiled hashes
/// live, so linking depends on it.
#[test]
fn re_exports_resolve_to_their_origin() {
    let mut registry = ModuleRegistry::new();

    let origin_path = ModulePath::from_str_segments(&["origin"]).unwrap();
    registry.register(
        &origin_path,
        Arc::new(Module {
            name: Arc::from("origin"),
            doc: None,
            items: vec![make_function("helper", true)],
        }),
    );

    let facade_path = ModulePath::from_str_segments(&["facade"]).unwrap();
    registry.register(
        &facade_path,
        Arc::new(Module {
            name: Arc::from("facade"),
            doc: None,
            items: use_items(UsePrefix::Pkg, &["origin"], &["helper"], true),
        }),
    );

    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &main_path,
        Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: use_items(UsePrefix::Pkg, &["facade"], &["helper"], false),
        }),
    );

    // lookup through the facade lands on the origin
    let (_, origin) = registry.lookup_symbol(&facade_path, "helper").unwrap();
    assert_eq!(origin, origin_path);

    // and resolve_imports records the origin as from_module
    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.errors.is_empty());
    match resolved.imports["helper"].as_slice() {
        [ResolvedImport::Symbol { from_module, .. }] => {
            assert_eq!(*from_module, origin_path);
        }
        other => panic!("Expected a single symbol import, got {other:?}"),
    }
}

/// Failed imports surface as errors instead of silently binding
/// nothing: missing symbols, private symbols, and missing modules.
#[test]
fn failed_imports_are_reported() {
    let mut registry = ModuleRegistry::new();

    let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
    registry.register(
        &utils_path,
        Arc::new(Module {
            name: Arc::from("utils"),
            doc: None,
            items: vec![make_function("secret", false)],
        }),
    );

    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    registry.register(
        &main_path,
        Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: [
                use_items(UsePrefix::Pkg, &["utils"], &["missing"], false),
                use_items(UsePrefix::Pkg, &["utils"], &["secret"], false),
                use_items(UsePrefix::Pkg, &["nonexistent"], &["anything"], false),
            ]
            .concat(),
        }),
    );

    let resolved = registry.resolve_imports(&main_path).unwrap();
    assert!(resolved.imports.is_empty());
    assert_eq!(resolved.errors.len(), 3);
    assert!(matches!(
        resolved.errors[0].error,
        RegistryError::SymbolNotFound { .. }
    ));
    assert!(matches!(
        resolved.errors[1].error,
        RegistryError::NotPublic { .. }
    ));
    assert!(matches!(
        resolved.errors[2].error,
        RegistryError::ModuleNotFound(_)
    ));
}
