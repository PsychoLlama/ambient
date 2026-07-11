//! Constants: compilation, hashing/dedup, name binding, and artifact-pack
//! round-trips. Split from `tests.rs` (per-file line budgets).

use std::sync::Arc;

use super::*;
use crate::ast::{BinaryOp, Expr, FunctionDef, Item, ItemKind, Module, Span};
use crate::value::Value;

fn test_span() -> Span {
    Span::default()
}

/// End-to-end: a module-level `const` referenced from a function body
/// compiles (its name resolves) and evaluates to the constant's value,
/// which is inlined at the reference site. The constant itself produces
/// no compiled function.
#[test]
fn module_const_compiles_and_evaluates() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;
    use crate::vm::Vm;

    // const NANOS_PER_SEC: number = 1_000_000_000
    // fn run() = NANOS_PER_SEC
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("NANOS_PER_SEC"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(1_000_000_000.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("NANOS_PER_SEC"),
                }),
                test_span(),
            ),
        ],
    };

    // Type-check first (the checker registers the const so the body
    // resolves), then compile the checked module.
    let checked = crate::infer::check_module(module);
    assert!(
        checked.errors.is_empty(),
        "unexpected type errors: {:?}",
        checked.errors
    );

    let compiled = compile_module(&checked.module).expect("compilation failed");

    // The constant is a standalone value object, not a function: only
    // `run` is a compiled function.
    assert_eq!(
        compiled.functions.len(),
        1,
        "constant should not produce a compiled function"
    );
    // It does produce a content-addressed value object.
    let value_objects = compiled
        .objects
        .values()
        .filter(|o| o.as_value().is_some())
        .count();
    assert_eq!(value_objects, 1, "constant should produce one value object");

    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    let result = vm.call(&entry, vec![]).expect("run failed");
    assert_eq!(result, Value::Number(1_000_000_000.0));
}

/// A `const` compiles to a content-addressed value object, and a
/// referencing function links to it by hash (`LoadObject` + a dependency
/// edge) rather than inlining the literal.
#[test]
fn const_reference_links_by_hash_not_inlined() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    // Exactly one value object, whose hash is a pure function of the value.
    let expected_hash = crate::object::value_object(&Value::Number(42.0))
        .unwrap()
        .hash();
    let value_hashes: Vec<_> = compiled
        .objects
        .iter()
        .filter(|(_, o)| o.as_value().is_some())
        .map(|(h, _)| *h)
        .collect();
    assert_eq!(value_hashes, vec![expected_hash]);

    // `run` records the const hash as a dependency and emits `LoadObject`
    // — no inlined `PushConst 42` at the reference site.
    let run = compiled.get_function("run").expect("run");
    assert!(
        run.dependencies.contains(&expected_hash),
        "const hash should be a dependency"
    );
    let listing = crate::bytecode::disassemble(run);
    assert!(
        listing.contains("LoadObject"),
        "reference should compile to LoadObject: {listing}"
    );
    assert!(
        !listing.contains("PushConst"),
        "the literal must not be inlined: {listing}"
    );
}

/// A `const` written without a type annotation type-checks (the type is
/// inferred from the literal) and compiles/runs like an annotated one.
#[test]
fn const_without_annotation_infers_type() {
    use crate::ast::ConstDef;
    use crate::vm::Vm;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: None,
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    let result = vm.call(&entry, vec![]).expect("run failed");
    assert_eq!(result, Value::Number(42.0));
}

/// A block-scoped `const` is content-addressed exactly like a module-level
/// one: a reference links to its value object by hash (`LoadObject` + a
/// dependency edge, so the value's hash is part of the function's
/// identity), and an identical module const collapses to the same object.
#[test]
fn block_const_links_by_hash_and_dedups_with_module_const() {
    use crate::ast::{ConstDef, ExprKind, Stmt, StmtKind};
    use crate::value::Value;
    use crate::vm::Vm;

    // A module const and a block const, both `7` — content addressing
    // must collapse them to one value object.
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("SHARED"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: None,
                    value: Expr::number(7.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::new(
                        ExprKind::Block(
                            vec![Stmt::new(
                                StmtKind::Const(ConstDef {
                                    id: 42,
                                    name: Arc::from("LOCAL"),
                                    name_span: Span::default(),
                                    is_public: false,
                                    ty: None,
                                    value: Expr::number(7.0),
                                }),
                                test_span(),
                            )],
                            Some(Box::new(Expr::name("LOCAL"))),
                        ),
                        test_span(),
                    ),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    // Both consts share one value object, keyed purely by content.
    let expected_hash = crate::object::value_object(&Value::Number(7.0))
        .unwrap()
        .hash();
    let value_hashes: Vec<_> = compiled
        .objects
        .iter()
        .filter(|(_, o)| o.as_value().is_some())
        .map(|(h, _)| *h)
        .collect();
    assert_eq!(value_hashes, vec![expected_hash]);

    // `run` links to it by hash: dependency edge + `LoadObject`, never an
    // inlined literal.
    let run = compiled.get_function("run").expect("run");
    assert!(
        run.dependencies.contains(&expected_hash),
        "block const hash should be a dependency of the referencing function"
    );
    let listing = crate::bytecode::disassemble(run);
    assert!(
        listing.contains("LoadObject"),
        "block const reference should compile to LoadObject: {listing}"
    );

    // And it actually runs from a cold VM.
    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    assert_eq!(vm.call(&entry, vec![]).expect("run"), Value::Number(7.0));
}

/// Two `const`s with the same value collapse to a single value object:
/// content addressing deduplicates them, name notwithstanding.
#[test]
fn identical_consts_deduplicate_to_one_object() {
    use crate::ast::ConstDef;
    use crate::types::Type;

    let mk_const = |name: &str| {
        Item::new(
            ItemKind::Const(ConstDef {
                id: 0,
                name: Arc::from(name),
                name_span: Span::default(),
                is_public: false,
                ty: Some(Type::number()),
                value: Expr::number(100.0),
            }),
            test_span(),
        )
    };
    // Reference both so neither is dead-code-eliminated conceptually
    // (the compiler keeps all consts regardless, but this mirrors use).
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            mk_const("A"),
            mk_const("B"),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::name("A"), Expr::name("B")),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let value_object_count = compiled
        .objects
        .values()
        .filter(|o| o.as_value().is_some())
        .count();
    assert_eq!(
        value_object_count, 1,
        "two consts with the same value share one object"
    );
}

/// A named `const` binds its short name to its value-object hash in
/// `const_names` (a first-class named binding for the store), and never
/// leaks into `function_names` — a const is not a function.
#[test]
fn const_name_binds_to_value_object_hash() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Const(ConstDef {
                id: 0,
                name: Arc::from("ANSWER"),
                name_span: Span::default(),
                is_public: true,
                ty: Some(Type::number()),
                value: Expr::number(42.0),
            }),
            test_span(),
        )],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let expected_hash = crate::object::value_object(&Value::Number(42.0))
        .unwrap()
        .hash();
    assert_eq!(
        compiled.const_names.get("ANSWER").copied(),
        Some(expected_hash),
        "const name should bind to its value-object hash"
    );
    assert!(
        !compiled.function_names.contains_key("ANSWER"),
        "a const must not appear in function_names"
    );
    // The bound hash addresses a `Value` object in the module.
    assert!(matches!(
        compiled.objects.get(&expected_hash),
        Some(crate::object::StoredObject::Value(_))
    ));
}

/// A pack round-trip preserves the function/const split: `from_pack`
/// routes each name back by the kind of object it binds (a `Value`
/// object ⇒ `const_names`, a function ⇒ `function_names`), even though
/// the pack itself carries one flat name list.
#[test]
fn pack_round_trip_preserves_const_names() {
    use crate::ast::ConstDef;
    use crate::types::Type;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: true,
                    ty: Some(Type::number()),
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let mut compiled = compile_module(&checked.module).expect("compile");
    // Signatures are attached at the check+compile seam by callers, and
    // migrations by `init_versioned` perform sites; stamp both here so the
    // round-trip below covers the deploy-metadata sections of the pack.
    compiled
        .signatures
        .insert(Arc::from("run"), Arc::from("() -> Number"));
    compiled.migrations.push(crate::compiler::MigrationRecord {
        cell: Arc::from("stats"),
        old: Arc::from("StatsV1"),
        new: Arc::from("Stats"),
    });

    let restored = CompiledModule::from_pack(&compiled.to_pack()).expect("from_pack");
    assert_eq!(
        restored.const_names.get("ANSWER").copied(),
        compiled.const_names.get("ANSWER").copied(),
        "const name should survive the pack round-trip in const_names"
    );
    assert!(
        restored.function_names.contains_key("run"),
        "function name should survive in function_names"
    );
    assert!(
        !restored.function_names.contains_key("ANSWER"),
        "a const must not be reconstructed as a function"
    );
    assert_eq!(
        restored.signatures.get("run").map(AsRef::as_ref),
        Some("() -> Number"),
        "signatures should survive the pack round-trip"
    );
    assert_eq!(
        restored.migrations, compiled.migrations,
        "migration records should survive the pack round-trip"
    );
}

/// A `const` initialized with a non-literal (here, a reference to another
/// name) is rejected by the type checker: constants map an identifier to a
/// single hashed primitive, so the initializer must be a literal.
#[test]
fn non_literal_const_is_rejected() {
    use crate::ast::ConstDef;
    use crate::infer::TypeErrorKind;
    use crate::types::Type;

    // const A: number = 1;
    // const B: number = A;   // not a literal
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("A"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(1.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("B"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::name("A"),
                }),
                test_span(),
            ),
        ],
    };

    let checked = crate::infer::check_module(module);
    assert!(
        checked
            .errors
            .iter()
            .any(|e| matches!(e.kind, TypeErrorKind::ConstNotLiteral { .. })),
        "expected a ConstNotLiteral error, got: {:?}",
        checked.errors
    );
}
