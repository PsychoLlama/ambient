use ambient_engine::ast::{BinaryOp, ExprKind, ItemKind};
use ambient_engine::types::Type;

use crate::error::ParseErrorKind;
use crate::parse;

#[test]
fn test_lower_simple_function() {
    let source = "fn add(x: Number, y: Number): Number { x + y }";
    let module = parse(source).expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        ItemKind::Function(f) => {
            assert_eq!(&*f.name, "add");
            assert_eq!(f.params.len(), 2);
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_lower_expression() {
    use crate::parse_expr;

    let expr = parse_expr("1 + 2 * 3").expect("parse error");
    match expr.kind {
        ExprKind::Binary {
            op: BinaryOp::Add, ..
        } => {}
        _ => panic!("Expected binary add expression"),
    }
}

#[test]
fn test_lower_if_expression() {
    use crate::parse_expr;

    let expr = parse_expr("if true { 1 } else { 2 }").expect("parse error");
    match expr.kind {
        ExprKind::If(cond, _then_br, else_br) => {
            assert!(matches!(cond.kind, ExprKind::Bool(true)));
            assert!(else_br.is_some());
        }
        _ => panic!("Expected if expression"),
    }
}

#[test]
fn test_lower_lambda() {
    use crate::parse_expr;

    let expr = parse_expr("(x) => x + 1").expect("parse error");
    match expr.kind {
        ExprKind::Lambda(lambda) => {
            assert_eq!(lambda.params.len(), 1);
            assert_eq!(&*lambda.params[0].name, "x");
        }
        _ => panic!("Expected lambda"),
    }
}

#[test]
fn test_lower_record() {
    use crate::parse_expr;

    let expr = parse_expr("{ x: 1, y: 2 }").expect("parse error");
    match expr.kind {
        ExprKind::Record(fields) => {
            assert_eq!(fields.len(), 2);
        }
        _ => panic!("Expected record"),
    }
}

#[test]
fn test_lower_enum() {
    let source = "unique(A1B2C3D4-0000-0000-0000-000000000001) enum Maybe<T> { Some(T), None }";
    let module = parse(source).expect("parse error");
    match &module.items[0].kind {
        ItemKind::Enum(e) => {
            assert_eq!(&*e.name, "Maybe");
            assert_eq!(e.type_params.len(), 1);
            assert_eq!(e.variants.len(), 2);
            assert_eq!(e.uuid.to_string(), "a1b2c3d4-0000-0000-0000-000000000001");
        }
        _ => panic!("Expected enum"),
    }
}

#[test]
fn test_lower_bare_enum_rejected() {
    // Every enum must carry a `unique(<uuid>)` prefix; a bare `enum` has
    // no nominal identity and is rejected at lowering.
    let source = "enum Color { Red, Green, Blue }";
    let err = parse(source).expect_err("bare enum should be rejected");
    assert!(matches!(err.kind, ParseErrorKind::EnumRequiresUnique));
}

#[test]
fn test_lower_function_with_doc_comment() {
    let source = "/// Adds two numbers.\nfn add(x: Number, y: Number): Number { x + y }";
    let module = parse(source).expect("parse error");
    assert_eq!(module.items.len(), 1);
    let doc = module.items[0].doc.as_ref().expect("Expected doc comment");
    assert_eq!(&**doc, "Adds two numbers.");
}

#[test]
fn test_lower_module_with_inner_doc() {
    let source = "//! Module documentation.\n\nfn foo() { () }";
    let module = parse(source).expect("parse error");
    let doc = module.doc.as_ref().expect("Expected module doc");
    assert_eq!(&**doc, "Module documentation.");
}

#[test]
fn test_lower_nominal_type() {
    let source = "unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: String }";
    let module = parse(source).expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        ItemKind::Struct(s) => {
            assert_eq!(&*s.name, "UserId");
            assert!(s.unique_id.is_some());
            let uuid = s.unique_id.unwrap();
            // Source syntax is uppercase; the canonical value is lowercase.
            assert_eq!(uuid.to_string(), "d098767b-4093-4d5c-ba37-ad92aa7b5d98");
            // The type should be wrapped in Nominal
            assert!(matches!(s.ty, Type::Nominal(_)));
        }
        _ => panic!("Expected struct"),
    }
}

#[test]
fn test_lower_nominal_type_uuid_with_exponent_like_group() {
    // Regression: a UUID whose first hex group is `<digit>E<hex letter>`
    // (here `2EB9553C`) once crashed the lexer, which mistook `2E...` for a
    // malformed scientific-notation literal. It is now lexed as a single
    // `Uuid` token and must validate as a real UUID like any other.
    let source = "unique(2EB9553C-1FDF-46FB-A8B1-F2C5A1CFCA94) struct Example { value: String }";
    let module = parse(source).expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        ItemKind::Struct(s) => {
            assert_eq!(&*s.name, "Example");
            let uuid = s.unique_id.expect("expected nominal type");
            assert_eq!(uuid.to_string(), "2eb9553c-1fdf-46fb-a8b1-f2c5a1cfca94");
            assert!(matches!(s.ty, Type::Nominal(_)));
        }
        _ => panic!("Expected struct"),
    }
}

#[test]
fn test_lower_regular_struct() {
    let source = "struct Point { x: Number, y: Number }";
    let module = parse(source).expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        ItemKind::Struct(s) => {
            assert_eq!(&*s.name, "Point");
            assert!(s.unique_id.is_none());
            // A non-unique struct is a bare record, NOT wrapped in Nominal.
            assert!(matches!(s.ty, Type::Record(_)));
        }
        _ => panic!("Expected struct"),
    }
}

#[test]
fn test_lower_invalid_uuid() {
    // Non-UUID content in `unique(...)` is now rejected at parse time (the
    // lexer only produces a `Uuid` token for canonical uppercase UUIDs),
    // so the error is `ExpectedUuid` rather than a lowering `InvalidUuid`.
    let source = "unique(not-a-valid-uuid) struct BadId { value: String }";
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err.kind, ParseErrorKind::ExpectedUuid));
}

#[test]
fn test_lower_lowercase_uuid_rejected() {
    // A lowercase UUID is not a UUID literal in Ambient; it must be
    // rejected rather than silently accepted as a non-nominal type.
    let source = "unique(2eb9553c-1fdf-46fb-a8b1-f2c5a1cfca94) struct BadId { value: String }";
    let result = parse(source);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::ExpectedUuid
    ));
}

#[test]
fn test_lower_bounded_type_params() {
    // `<T: Eq + Ord, U>` — bounds parse per parameter and lower to
    // qualified names on the AST TypeParam.
    let source = "fn max_of<T: Eq + Ord, U>(a: T, b: T, tag: U): T { a }";
    let module = parse(source).expect("parse error");
    match &module.items[0].kind {
        ItemKind::Function(f) => {
            assert_eq!(f.type_params.len(), 2);
            let t = &f.type_params[0];
            assert_eq!(&*t.name, "T");
            let bounds: Vec<&str> = t.bounds.iter().map(|b| &*b.name.name).collect();
            assert_eq!(bounds, ["Eq", "Ord"]);
            assert!(f.type_params[1].bounds.is_empty());
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_lower_ability_variable_param() {
    // `<T, E!>` — the `!` suffix lowers to `is_ability` on the AST
    // TypeParam; ordinary parameters stay `is_ability: false`.
    let source = "fn run<T, E!>(t: () -> T with E): T with E { t() }";
    let module = parse(source).expect("parse error");
    match &module.items[0].kind {
        ItemKind::Function(f) => {
            assert_eq!(f.type_params.len(), 2);
            assert!(!f.type_params[0].is_ability, "`T` is a type variable");
            assert!(f.type_params[1].is_ability, "`E!` is an ability variable");
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_lower_ability_variable_with_bounds_is_error() {
    // An ability variable names an effect row, not a type, so it may carry
    // no trait bounds.
    let source = "fn bad<E!: Eq>(t: () -> Number with E): Number with E { t() }";
    let err = parse(source).expect_err("expected a lowering error");
    match err.kind {
        ParseErrorKind::LoweringError(msg) => {
            assert!(msg.contains("cannot have trait bounds"), "got: {msg}");
        }
        other => panic!("expected LoweringError, got {other:?}"),
    }
}

#[test]
fn test_lower_impl_where_clause_folds_into_bounds() {
    // A trailing `where T: Eq` is the same declaration as `impl<T: Eq>`:
    // lowering folds it into the parameter, so there is exactly one AST
    // representation for bounds.
    let source = r"
        unique(A1B2C3D4-0000-0000-0000-000000000001) struct Box2 { v: Number }
        impl<T> Box2 where T: Eq {
            fn get(self): Number { self.v }
        }
    ";
    let module = parse(source).expect("parse error");
    match &module.items[1].kind {
        ItemKind::Impl(i) => {
            assert_eq!(i.type_params.len(), 1);
            let bounds: Vec<&str> = i.type_params[0].bounds.iter().map(|b| &*b.name.name).collect();
            assert_eq!(bounds, ["Eq"]);
        }
        _ => panic!("Expected impl"),
    }
}

#[test]
fn test_lower_fn_where_clause_folds_into_bounds() {
    // A fn-level `where T: Ord` is the same declaration as `fn f<T: Ord>`:
    // it folds into the type parameter's bounds, so there is one AST shape.
    let source = "fn cmp_them<T>(a: T, b: T): Number where T: Ord { a.cmp(b) }";
    let module = parse(source).expect("parse error");
    match &module.items[0].kind {
        ItemKind::Function(f) => {
            assert_eq!(f.type_params.len(), 1);
            let bounds: Vec<&str> = f.type_params[0].bounds.iter().map(|b| &*b.name.name).collect();
            assert_eq!(bounds, ["Ord"]);
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_lower_fn_where_before_with() {
    // `where` precedes the `with` effect clause: the two never clash and
    // both bounds and abilities survive lowering.
    let source = "fn f<T>(x: T): T where T: Eq + Ord with core::system::Stdio { x }";
    let module = parse(source).expect("parse error");
    match &module.items[0].kind {
        ItemKind::Function(f) => {
            let bounds: Vec<&str> = f.type_params[0].bounds.iter().map(|b| &*b.name.name).collect();
            assert_eq!(bounds, ["Eq", "Ord"]);
            assert_eq!(f.abilities.len(), 1);
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_lower_fn_where_clause_on_non_param_rejected() {
    // A fn `where` clause may only constrain the fn's own type parameters.
    let source = "fn f<T>(x: T): T where Number: Eq { x }";
    let result = parse(source);
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::LoweringError(_)
    ));
}

#[test]
fn test_lower_where_clause_on_non_param_rejected() {
    // `where` can only constrain the impl's own type parameters.
    let source = r"
        unique(A1B2C3D4-0000-0000-0000-000000000001) struct Box2 { v: Number }
        impl<T> Box2 where Number: Eq {
            fn get(self): Number { self.v }
        }
    ";
    let result = parse(source);
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::LoweringError(_)
    ));
}

#[test]
fn test_lower_bounds_on_struct_params_rejected() {
    // Type declarations carry no code, so bounds there are meaningless.
    let source = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct Pair<T: Eq> { a: T, b: T }";
    let result = parse(source);
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::LoweringError(_)
    ));
}

#[test]
fn test_lower_bounds_on_extern_fn_rejected() {
    // Natives have no dictionary calling convention.
    let source = "extern fn find<T: Eq>(items: List<T>, needle: T): Bool;";
    let result = parse(source);
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::LoweringError(_)
    ));
}

#[test]
fn test_lower_trait_requires_unique() {
    // Traits are nominal: a bare `trait` has no identity.
    let source = "trait Show { fn show(self): String; }";
    let result = parse(source);
    assert!(matches!(
        result.unwrap_err().kind,
        ParseErrorKind::TraitRequiresUnique
    ));
}
