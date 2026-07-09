use std::sync::Arc;

use crate::types::Type;

use super::{check_module, resolve_ability_declarations};

fn span() -> crate::ast::Span {
    crate::ast::Span { start: 0, end: 0 }
}

fn method(
    name: &str,
    type_params: &[&str],
    params: &[(&str, Type)],
    ret_ty: Type,
) -> crate::ast::AbilityMethod {
    crate::ast::AbilityMethod {
        name: Arc::from(name),
        type_params: type_params
            .iter()
            .map(|name| crate::ast::TypeParam {
                name: Arc::from(*name),
                span: span(),
            })
            .collect(),
        params: params
            .iter()
            .map(|(name, ty)| (Arc::from(*name), ty.clone()))
            .collect(),
        ret_ty,
        span: span(),
    }
}

fn ability_module(name: &str, methods: Vec<crate::ast::AbilityMethod>) -> crate::ast::Module {
    crate::ast::Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![crate::ast::Item {
            kind: crate::ast::ItemKind::Ability(crate::ast::AbilityDef {
                name: Arc::from(name),
                name_span: span(),
                is_public: true,
                dependencies: vec![],
                methods,
                resolved_id: None,
            }),
            span: span(),
            doc: None,
        }],
    }
}

/// A named type-parameter reference as the parser lowers it.
fn ty_param(name: &str) -> Type {
    Type::Named(crate::types::NamedType::new(Arc::from(name), vec![]))
}

/// An in-language declaration must hash to the same identity as a
/// descriptor-style rendering of the interface (method IDs are
/// declaration indices, signatures render through the canonical type
/// grammar): this is what lets host handlers keyed against the
/// resolved declarations serve performs compiled from them.
#[test]
fn declaration_hashing_matches_descriptor_hashing() {
    use ambient_core::{MethodDescriptor, hash_interface};

    let mut module = ability_module(
        "Console",
        vec![
            method("print", &[], &[("message", Type::string())], Type::Unit),
            method("eprint", &[], &[("message", Type::string())], Type::Unit),
            method("println", &[], &[("message", Type::string())], Type::Unit),
        ],
    );

    // Types here are already resolved nominals (`Type::string()`), so no
    // prelude seeding is needed — an empty registry suffices.
    let (abilities, errors) =
        resolve_ability_declarations(&mut module, &crate::module_registry::ModuleRegistry::new());
    assert!(errors.is_empty());
    assert_eq!(abilities.len(), 1);

    let expected = hash_interface(
        "Console",
        &[
            MethodDescriptor::new(0, "print", 1, |f| vec![f.string()], |f| f.unit()),
            MethodDescriptor::new(1, "eprint", 1, |f| vec![f.string()], |f| f.unit()),
            MethodDescriptor::new(2, "println", 1, |f| vec![f.string()], |f| f.unit()),
        ],
    );

    let console = &abilities[0];
    assert_eq!(console.id, expected);
    assert_eq!(console.method("print").map(|m| m.id), Some(0));
    assert_eq!(console.method("eprint").map(|m| m.id), Some(1));
    assert_eq!(console.method("println").map(|m| m.id), Some(2));

    // The identity is also written back for the compiler.
    let crate::ast::ItemKind::Ability(def) = &module.items[0].kind else {
        panic!("expected ability item");
    };
    assert_eq!(def.resolved_id, Some(console.id));
}

/// Generic methods are the risky parity case: the descriptor renders
/// each `type_var()` occurrence as an independent `varN`, so the
/// declaration must use a distinct type parameter per position
/// (`run<T, R>` — never one parameter in two positions).
#[test]
fn generic_declaration_hashing_matches_descriptor_hashing() {
    use ambient_core::{MethodDescriptor, hash_interface};

    let list_of_string = Type::named("List", vec![Type::string()]);
    let mut module = ability_module(
        "Execute",
        vec![
            method(
                "has_function",
                &[],
                &[("hash", Type::string())],
                Type::bool(),
            ),
            method(
                "get_dependencies",
                &[],
                &[("hash", Type::string())],
                list_of_string.clone(),
            ),
            method(
                "load_functions",
                &[],
                &[("bundle", Type::binary())],
                Type::Unit,
            ),
            method(
                "run",
                &["T", "R"],
                &[("hash", Type::string()), ("args", ty_param("T"))],
                ty_param("R"),
            ),
            method(
                "get_functions",
                &[],
                &[("hashes", list_of_string)],
                Type::binary(),
            ),
            method(
                "run_with",
                &["T", "U", "R"],
                &[
                    ("hash", Type::string()),
                    ("args", ty_param("T")),
                    ("handler", ty_param("U")),
                ],
                ty_param("R"),
            ),
        ],
    );

    let (abilities, errors) =
        resolve_ability_declarations(&mut module, &crate::module_registry::ModuleRegistry::new());
    assert!(errors.is_empty());

    let expected = hash_interface(
        "Execute",
        &[
            MethodDescriptor::new(0, "has_function", 1, |f| vec![f.string()], |f| f.bool()),
            MethodDescriptor::new(
                1,
                "get_dependencies",
                1,
                |f| vec![f.string()],
                |f| f.list(f.string()),
            ),
            MethodDescriptor::new(2, "load_functions", 1, |f| vec![f.binary()], |f| f.unit()),
            MethodDescriptor::new(
                3,
                "run",
                2,
                |f| vec![f.string(), f.type_var()],
                |f| f.type_var(),
            ),
            MethodDescriptor::new(
                4,
                "get_functions",
                1,
                |f| vec![f.list(f.string())],
                |f| f.binary(),
            ),
            MethodDescriptor::new(
                5,
                "run_with",
                3,
                |f| vec![f.string(), f.type_var(), f.type_var()],
                |f| f.type_var(),
            ),
        ],
    );

    let execute = &abilities[0];
    assert_eq!(execute.id, expected);
    assert_eq!(execute.method("run").map(|m| m.id), Some(3));
    assert_eq!(execute.method("run_with").map(|m| m.id), Some(5));
}

/// A module-level `const` must be in scope inside function bodies,
/// regardless of declaration order. Regression test: consts used to be
/// checked (their value against their annotation) but never registered
/// into the module environment, so a reference from a function body
/// resolved to `UndefinedVariable`.
#[test]
fn module_const_is_in_scope_in_function_bodies() {
    use crate::ast::{ConstDef, Expr, FunctionDef, Item, ItemKind, Module};

    // `fn use_it() = NANOS_PER_SEC + 1` is declared *before* the const it
    // references, to also cover forward references.
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("use_it"),
                    name_span: span(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(
                        crate::ast::BinaryOp::Add,
                        Expr::name("NANOS_PER_SEC"),
                        Expr::number(1.0),
                    ),
                }),
                span(),
            ),
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("NANOS_PER_SEC"),
                    name_span: span(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(1_000_000_000.0),
                }),
                span(),
            ),
        ],
    };

    let result = check_module(module);
    assert!(
        result.errors.is_empty(),
        "const reference from a function body should type-check, got: {:?}",
        result.errors
    );
}

#[test]
fn registry_less_check_resolves_a_local_enum_by_bare_name() {
    use crate::ast::{
        EnumDef, EnumVariant, Expr, FunctionDef, Item, ItemKind, MatchArm, Module, Param, Pattern,
    };

    // `check_module` runs no resolve pass, so a bare enum annotation and a
    // bare variant pattern arrive uuid-less and unresolved. This proves the
    // checker's own bare-name discovery — the registry-less fallback the
    // resolve pass supersedes on every real check — is still intact:
    //
    //   enum E { V }
    //   fn f(x: E): Number { match x { V => 1 } }
    //
    // The parameter annotation `E` resolves through `resolve_holes`'s
    // `enum_registry` stamp, and the pattern `V` through `resolve_variant`.
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Enum(EnumDef {
                    name: Arc::from("E"),
                    name_span: span(),
                    is_public: false,
                    type_params: vec![],
                    variants: vec![EnumVariant {
                        name: Arc::from("V"),
                        payload: None,
                        span: span(),
                    }],
                    uuid: uuid::Uuid::from_u128(0x5151),
                }),
                span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("f"),
                    name_span: span(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param {
                        id: 0,
                        name: Arc::from("x"),
                        ty: Some(Type::named_simple("E")),
                        span: span(),
                    }],
                    ret_ty: Some(Type::number()),
                    abilities: vec![],
                    body: Expr::match_expr(
                        Expr::name("x"),
                        vec![MatchArm {
                            pattern: Pattern::variant("V", None),
                            guard: None,
                            body: Expr::number(1.0),
                        }],
                    ),
                }),
                span(),
            ),
        ],
    };

    let result = check_module(module);
    assert!(
        result.errors.is_empty(),
        "a registry-less check of a local enum annotation/pattern should type-check, got: {:?}",
        result.errors
    );
}
