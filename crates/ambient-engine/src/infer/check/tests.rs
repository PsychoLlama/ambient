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
                is_ability: false,
                bounds: Vec::new(),
                span: span(),
            })
            .collect(),
        params: params
            .iter()
            .enumerate()
            .map(|(i, (name, ty))| crate::ast::Param {
                id: crate::ast::BindingId::try_from(i).expect("test param count"),
                name: Arc::from(*name),
                ty: Some(ty.clone()),
                span: span(),
            })
            .collect(),
        ret_ty,
        body: None,
        resolved_signature: None,
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
                uuid: uuid::Uuid::from_u128(0xA1B2_C3D4),
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

/// An in-language declaration's identity is its uuid — not its name or
/// shape — and each method's canonical signature hash is deterministic:
/// same-signature methods share it, different signatures do not. This is
/// what lets a foreign import recompute the exact identity the declaring
/// module registered.
#[test]
fn declaration_identity_is_the_uuid() {
    let mut module = ability_module(
        "Console",
        vec![
            method("print", &[], &[("message", Type::string())], Type::Unit),
            method("eprint", &[], &[("message", Type::string())], Type::Unit),
            method("println", &[], &[("count", Type::number())], Type::Unit),
        ],
    );

    // Types here are already resolved nominals (`Type::string()`), so no
    // prelude seeding is needed — an empty registry suffices.
    let (abilities, errors) =
        resolve_ability_declarations(&mut module, &crate::module_registry::ModuleRegistry::new());
    assert!(errors.is_empty());
    assert_eq!(abilities.len(), 1);

    let console = &abilities[0];
    assert_eq!(
        console.id,
        crate::types::AbilityId::from_uuid(&uuid::Uuid::from_u128(0xA1B2_C3D4))
    );
    assert_eq!(console.uuid, uuid::Uuid::from_u128(0xA1B2_C3D4));

    // Same signature, different names: identical signature hashes — names
    // never enter method identity (renames are free); the implementation
    // hash is what distinguishes them at compile time.
    let print_sig = console.method("print").map(|m| m.signature);
    assert_eq!(print_sig, console.method("eprint").map(|m| m.signature));
    assert_ne!(print_sig, console.method("println").map(|m| m.signature));
    assert_eq!(
        print_sig,
        Some(ambient_core::SignatureHash::new(&["string"], "unit"))
    );

    // The identity and signatures are also written back for the compiler.
    let crate::ast::ItemKind::Ability(def) = &module.items[0].kind else {
        panic!("expected ability item");
    };
    assert_eq!(def.resolved_id, Some(console.id));
    assert_eq!(def.methods[0].resolved_signature, print_sig);
}

/// Generic methods canonicalize type variables by first occurrence per
/// signature, so `run<T, R>(hash: String, args: T): R` renders identically
/// regardless of what checked before it — signature hashes stay stable.
#[test]
fn generic_signature_hashes_are_position_canonical() {
    let mut module = ability_module(
        "Execute",
        vec![method(
            "run",
            &["T", "R"],
            &[("hash", Type::string()), ("args", ty_param("T"))],
            ty_param("R"),
        )],
    );

    let (abilities, errors) =
        resolve_ability_declarations(&mut module, &crate::module_registry::ModuleRegistry::new());
    assert!(errors.is_empty());

    let execute = &abilities[0];
    assert_eq!(
        execute.method("run").map(|m| m.signature),
        Some(ambient_core::SignatureHash::new(
            &["string", "var0"],
            "var1"
        ))
    );
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

/// `type Bottom = !;` used as an ability method's return type must behave
/// exactly like a spelled `!`. A registry-backed check gets this from the
/// resolve pass (which inlines non-generic aliases into the AST), but a
/// registry-less check never runs resolve — the checker itself must derive
/// never-ness from the *resolved* signature. Pins three things: the
/// abstract carve-out applies (no "needs a default implementation" error),
/// the AST return type is normalized to `Type::Never` (the compiler's
/// unwind flag reads the AST), and the canonical signature hash is
/// identical to the spelled-`!` form.
#[test]
fn alias_of_never_in_ability_signature_behaves_like_never() {
    use crate::ast::{Item, ItemKind, TypeAliasDef};

    let mut module = ability_module(
        "Abort",
        vec![method(
            "abort",
            &[],
            &[("code", Type::number())],
            Type::named_simple("Bottom"),
        )],
    );
    module.items.insert(
        0,
        Item::new(
            ItemKind::TypeAlias(TypeAliasDef {
                name: Arc::from("Bottom"),
                name_span: span(),
                is_public: false,
                type_params: vec![],
                ty: Type::Never,
            }),
            span(),
        ),
    );
    let result = check_module(module);
    assert!(
        result.errors.is_empty(),
        "an abstract method returning an alias of `!` should check, got: {:?}",
        result.errors
    );

    let ItemKind::Ability(def) = &result.module.items[1].kind else {
        panic!("expected ability item");
    };
    assert!(
        matches!(def.methods[0].ret_ty, Type::Never),
        "return type should be normalized to `!`, got: {:?}",
        def.methods[0].ret_ty
    );
    assert_eq!(
        def.methods[0].resolved_signature,
        Some(ambient_core::SignatureHash::new(&["number"], "never")),
        "alias spelling must hash identically to a spelled `!`"
    );
}
