// These tests assert that number literals parse to exact, representable
// values (42.0, 1.0, ...), so exact float comparison is intended.
#![allow(clippy::float_cmp)]

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{
    CstBinaryOp, CstExpr, CstExprKind, CstIdent, CstItemKind, CstStmtKind, CstStructDef, CstUnaryOp,
};

#[test]
fn test_parse_number() {
    let mut parser = Parser::new("42").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Number(n) if n == 42.0));
}

#[test]
fn test_parse_string() {
    let mut parser = Parser::new(r#""hello""#).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(&expr.kind, CstExprKind::String(s) if &**s == "hello"));
}

#[test]
fn test_parse_bool() {
    let mut parser = Parser::new("true").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Bool(true)));

    let mut parser = Parser::new("false").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Bool(false)));
}

#[test]
fn test_parse_binary_expr() {
    let mut parser = Parser::new("1 + 2 * 3").unwrap();
    let expr = parser.parse_expression().expect("parse error");

    // Should parse as 1 + (2 * 3)
    match expr.kind {
        CstExprKind::Binary { op, left, right } => {
            assert_eq!(op, CstBinaryOp::Add);
            assert!(matches!(left.kind, CstExprKind::Number(n) if n == 1.0));
            match right.kind {
                CstExprKind::Binary { op, left, right } => {
                    assert_eq!(op, CstBinaryOp::Mul);
                    assert!(matches!(left.kind, CstExprKind::Number(n) if n == 2.0));
                    assert!(matches!(right.kind, CstExprKind::Number(n) if n == 3.0));
                }
                _ => panic!("Expected binary expression"),
            }
        }
        _ => panic!("Expected binary expression"),
    }
}

#[test]
fn test_parse_unary_expr() {
    let mut parser = Parser::new("-42").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Unary { op, operand } => {
            assert_eq!(op, CstUnaryOp::Neg);
            assert!(matches!(operand.kind, CstExprKind::Number(n) if n == 42.0));
        }
        _ => panic!("Expected unary expression"),
    }
}

#[test]
fn test_struct_and_type_alias_split() {
    // `struct` defines a record.
    let mut parser = Parser::new("struct Point { x: Number, y: Number }").unwrap();
    let item = parser.parse_item().expect("struct should parse");
    assert!(matches!(item.kind, CstItemKind::Struct(_)));

    // `unique(...) struct` defines a nominal record.
    let mut parser =
        Parser::new("unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Id { value: Number }")
            .unwrap();
    let item = parser.parse_item().expect("unique struct should parse");
    assert!(matches!(item.kind, CstItemKind::Struct(_)));

    // `unique(...) struct Foo;` defines a nominal unit struct (no body).
    let mut parser =
        Parser::new("unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Marker;").unwrap();
    let item = parser.parse_item().expect("unit struct should parse");
    assert!(matches!(item.kind, CstItemKind::Struct(_)));

    // `type X = Y` remains a plain alias.
    let mut parser = Parser::new("type Meters = Number;").unwrap();
    let item = parser.parse_item().expect("type alias should parse");
    assert!(matches!(item.kind, CstItemKind::TypeAlias(_)));

    // The old record-via-`type` form is now a parse error (requires `=`).
    let mut parser = Parser::new("type Point { x: Number }").unwrap();
    assert!(parser.parse_item().is_err(), "`type Name {{ }}` must error");
}

#[test]
fn test_unit_struct_lowering_rules() {
    use crate::error::ParseErrorKind;
    use crate::lower_module;

    // A bare unit struct (no `unique`) is rejected at lowering.
    let mut parser = Parser::new("struct Foo;").unwrap();
    let module = parser.parse_module().expect("unit struct should parse");
    let err = lower_module(&module).expect_err("bare unit struct must fail lowering");
    assert!(matches!(err.kind, ParseErrorKind::UnitStructRequiresUnique));

    // An empty brace body is rejected, pointing at the unit form.
    let mut parser = Parser::new("struct Foo {}").unwrap();
    let module = parser.parse_module().expect("empty struct should parse");
    let err = lower_module(&module).expect_err("empty-brace struct must fail lowering");
    assert!(matches!(err.kind, ParseErrorKind::EmptyStructBody));

    // Even with `unique`, an empty brace body is rejected.
    let mut parser =
        Parser::new("unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Foo {}").unwrap();
    let module = parser.parse_module().expect("empty struct should parse");
    let err = lower_module(&module).expect_err("empty-brace struct must fail lowering");
    assert!(matches!(err.kind, ParseErrorKind::EmptyStructBody));

    // A `unique` unit struct lowers successfully.
    let mut parser =
        Parser::new("unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Marker;").unwrap();
    let module = parser.parse_module().expect("unit struct should parse");
    lower_module(&module).expect("unique unit struct must lower");
}

#[test]
fn test_extern_struct_parsing_and_lowering() {
    use crate::error::ParseErrorKind;
    use crate::lower_module;

    // `extern unique(...) struct T;` parses and lowers.
    let mut parser =
        Parser::new("extern unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Token;").unwrap();
    let item = parser
        .parse_item()
        .expect("extern unit struct should parse");
    match item.kind {
        CstItemKind::Struct(ref s) => assert!(s.is_extern, "struct should be extern"),
        _ => panic!("expected a struct item"),
    }
    let mut parser =
        Parser::new("extern unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Token;").unwrap();
    let module = parser.parse_module().expect("extern struct should parse");
    lower_module(&module).expect("extern unit struct must lower");

    // A field-bearing extern struct is also legal.
    let mut parser = Parser::new(
        "extern unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Handle { id: Number }",
    )
    .unwrap();
    let module = parser.parse_module().expect("extern struct should parse");
    lower_module(&module).expect("field-bearing extern struct must lower");

    // `pub extern unique(...) struct T;` parses.
    let mut parser =
        Parser::new("pub extern unique(F6A7B8C9-D0E1-2345-FABC-456789012345) struct Token;")
            .unwrap();
    let item = parser.parse_item().expect("pub extern struct should parse");
    assert!(matches!(item.kind, CstItemKind::Struct(_)));

    // `extern struct T` without `unique(...)` is rejected. The parser
    // rejects it (no `unique` after `extern`) before lowering can run.
    let mut parser = Parser::new("extern struct Token;").unwrap();
    assert!(
        parser.parse_item().is_err(),
        "`extern` without `unique` must error"
    );

    // A recovered/hand-built extern struct without `unique` is rejected at
    // lowering with `ExternStructRequiresUnique`. Build the CST directly
    // since the parser refuses the `unique`-less form.
    let cst = CstStructDef {
        is_public: false,
        name: CstIdent {
            name: "Token".into(),
            span: Span::new(0, 0),
            trailing_trivia: crate::cst::Trivia::default(),
        },
        type_params: Vec::new(),
        ty: None,
        unique_id: None,
        is_extern: true,
    };
    let err = crate::lower::lower_struct_def(&cst)
        .expect_err("extern struct without unique must fail lowering");
    assert!(matches!(
        err.kind,
        ParseErrorKind::ExternStructRequiresUnique
    ));

    // `extern unique(...) enum E` is rejected — `extern` applies to structs.
    let mut parser =
        Parser::new("extern unique(F6A7B8C9-D0E1-2345-FABC-456789012345) enum Color { Red, Blue }")
            .unwrap();
    assert!(
        parser.parse_item().is_err(),
        "`extern` on an enum must error"
    );
}

#[test]
fn test_extern_fn_parsing_and_lowering() {
    use crate::error::ParseErrorKind;
    use crate::lower_module;
    use ambient_engine::ast::ItemKind;

    // A body-less signature parses and lowers.
    let mut parser = Parser::new("extern fn length(value: String): Number;").unwrap();
    let item = parser.parse_item().expect("extern fn should parse");
    assert!(matches!(item.kind, CstItemKind::ExternFn(_)));

    // `pub extern fn` with generics lowers to an ExternFn item.
    let mut parser = Parser::new("pub extern fn to_string<T>(value: T): String;").unwrap();
    let module = parser.parse_module().expect("pub extern fn should parse");
    let lowered = lower_module(&module).expect("extern fn must lower");
    match &lowered.items[0].kind {
        ItemKind::ExternFn(def) => {
            assert!(def.is_public);
            assert_eq!(def.name.as_ref(), "to_string");
            assert_eq!(def.type_params.len(), 1);
            assert_eq!(def.params.len(), 1);
        }
        other => panic!("expected an extern fn item, got {other:?}"),
    }

    // Zero-parameter extern fns are legal (`map::empty`-style constructors).
    let mut parser = Parser::new("extern fn empty<K, V>(): Map<K, V>;").unwrap();
    let module = parser.parse_module().expect("zero-arity extern fn parses");
    lower_module(&module).expect("zero-arity extern fn must lower");

    // A body instead of `;` is a parse error.
    let mut parser = Parser::new("extern fn f(x: Number): Number { x }").unwrap();
    assert!(parser.parse_item().is_err(), "extern fn body must error");

    // A `with` clause is rejected — extern fns are pure by construction.
    let mut parser =
        Parser::new("extern fn f(x: Number): Number with core::system::Stdio;").unwrap();
    let err = parser.parse_item().expect_err("with clause must error");
    assert!(matches!(err.kind, ParseErrorKind::ExternFnWithAbilities));

    // A missing return type is rejected at lowering (no body to infer from).
    let mut parser = Parser::new("extern fn f(x: Number);").unwrap();
    let module = parser.parse_module().expect("should parse");
    let err = lower_module(&module).expect_err("missing return type must fail");
    assert!(matches!(
        err.kind,
        ParseErrorKind::ExternFnRequiresReturnType
    ));

    // An untyped parameter is rejected at lowering.
    let mut parser = Parser::new("extern fn f(x): Number;").unwrap();
    let module = parser.parse_module().expect("should parse");
    let err = lower_module(&module).expect_err("untyped param must fail");
    assert!(matches!(
        err.kind,
        ParseErrorKind::ExternFnParamRequiresType(_)
    ));

    // `extern` followed by anything else still errors.
    let mut parser = Parser::new("extern const X: Number = 1;").unwrap();
    assert!(parser.parse_item().is_err(), "extern const must error");
}

#[test]
fn test_parse_if_expr() {
    let mut parser = Parser::new("if x { 1 } else { 2 }").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::If { .. }));
}

#[test]
fn test_parse_lambda() {
    let mut parser = Parser::new("(x) => x + 1").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Lambda(lambda) => {
            assert_eq!(lambda.params.len(), 1);
            assert_eq!(&*lambda.params[0].name.name, "x");
        }
        _ => panic!("Expected lambda"),
    }
}

#[test]
fn test_lambda_return_type_annotation_is_not_lambda_syntax() {
    // A `: Type` between the params `)` and `=>` is not lambda grammar: the
    // lambda lookahead only fires when `=>` immediately follows the closing
    // `)`. `(x): Number => x + 1` therefore parses `(x)` as a parenthesized
    // expression, and the trailing `: Number => ...` is a stray colon that
    // makes the enclosing `let` statement a parse error. (Pin a type to a
    // lambda via the binding annotation instead: `let f: (Number) -> Number
    // = x => x + 1;`.)
    let mut parser = Parser::new("let f = (x): Number => x + 1;").unwrap();
    assert!(
        parser.parse_module().is_err(),
        "annotated lambda syntax must be a parse error, not a lambda"
    );

    // A plain `(x) => x + 1` still parses as a lambda.
    let mut parser = Parser::new("(x) => x + 1").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Lambda(_)));
}

#[test]
fn test_parse_tuple_with_lambda_first_element() {
    // Regression: a tuple whose first element is a lambda must not be
    // misread as a lambda header. `(() => 2, 40)` is a 2-tuple.
    let mut parser = Parser::new("(() => 2, 40)").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Tuple(elements) => {
            assert_eq!(elements.len(), 2);
            assert!(matches!(elements[0].kind, CstExprKind::Lambda(_)));
        }
        other => panic!("Expected tuple, got {other:?}"),
    }
}

#[test]
fn test_parse_parenthesized_lambda() {
    // `(() => 2)` is a parenthesized lambda, not a lambda header.
    let mut parser = Parser::new("(() => 2)").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Lambda(_)));
}

#[test]
fn test_parse_lambda_with_parenthesized_param_types() {
    // Params containing parenthesized function types must still be depth
    // matched so the trailing `=>` is found.
    let mut parser = Parser::new("(f: (Number) -> Number) => f(1)").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Lambda(lambda) => {
            assert_eq!(lambda.params.len(), 1);
            assert_eq!(&*lambda.params[0].name.name, "f");
        }
        other => panic!("Expected lambda, got {other:?}"),
    }
}

#[test]
fn test_parse_call_with_tuple_lambda_arg() {
    // The reported repro: a call whose argument is a tuple with a lambda
    // first element.
    let mut parser = Parser::new("call_both((() => 2, 40))").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Call { .. }));
}

#[test]
fn test_parse_function() {
    let source = "fn add(x: Number, y: Number): Number { x + y }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        CstItemKind::Function(f) => {
            assert_eq!(&*f.name.name, "add");
            assert!(!f.is_public);
            assert_eq!(f.params.len(), 2);
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_parse_pub_function() {
    let source = "pub fn run(): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    assert_eq!(module.items.len(), 1);
    match &module.items[0].kind {
        CstItemKind::Function(f) => {
            assert_eq!(&*f.name.name, "run");
            assert!(f.is_public);
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_parse_function_with_abilities() {
    let source = "fn read_file(path: String): String with Filesystem { path }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    match &module.items[0].kind {
        CstItemKind::Function(f) => {
            assert_eq!(f.abilities.len(), 1);
            assert_eq!(&*f.abilities[0].segments[0].name, "Filesystem");
        }
        _ => panic!("Expected function"),
    }
}

#[test]
fn test_parse_enum() {
    let source = "enum Option<T> { Some(T), None }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    match &module.items[0].kind {
        CstItemKind::Enum(e) => {
            assert_eq!(&*e.name.name, "Option");
            assert_eq!(e.type_params.len(), 1);
            assert_eq!(e.variants.len(), 2);
        }
        _ => panic!("Expected enum"),
    }
}

#[test]
fn test_parse_ability_def() {
    let source = "unique(AB000000-0000-0000-0000-000000000017) ability Console { fn print(message: String): () { () } }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    match &module.items[0].kind {
        CstItemKind::Ability(a) => {
            assert_eq!(&*a.name.name, "Console");
            assert_eq!(a.methods.len(), 1);
            assert_eq!(&*a.methods[0].name.name, "print");
        }
        _ => panic!("Expected ability"),
    }
}

#[test]
fn test_parse_record_literal() {
    let mut parser = Parser::new("{ x: 1, y: 2 }").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Record(fields) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(&*fields[0].0.name, "x");
            assert_eq!(&*fields[1].0.name, "y");
        }
        _ => panic!("Expected record"),
    }
}

#[test]
fn test_parse_list_literal() {
    let mut parser = Parser::new("[1, 2, 3]").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::List(elements) => {
            assert_eq!(elements.len(), 3);
        }
        _ => panic!("Expected list"),
    }
}

#[test]
fn test_parse_match() {
    let source = r"
        match x {
            Some(v) => v,
            None => 0,
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Match { arms, .. } => {
            assert_eq!(arms.len(), 2);
        }
        _ => panic!("Expected match"),
    }
}

#[test]
fn test_parse_tuple() {
    let mut parser = Parser::new("(1, 2, 3)").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Tuple(elements) => {
            assert_eq!(elements.len(), 3);
        }
        _ => panic!("Expected tuple"),
    }
}

#[test]
fn test_parse_unit() {
    let mut parser = Parser::new("()").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    assert!(matches!(expr.kind, CstExprKind::Unit));
}

#[test]
fn test_parse_block() {
    let source = r"
        {
            let x = 1;
            let y = 2;
            x + y
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Block { stmts, result } => {
            assert_eq!(stmts.len(), 2);
            assert!(result.is_some());
        }
        _ => panic!("Expected block"),
    }
}

#[test]
fn test_parse_block_bodied_expr_in_statement_position() {
    // A block-bodied expression (`if`, `match`, …) followed by more code
    // is a statement — no semicolon required, Rust-style.
    let source = r"
        {
            if (x < 0) {
                boom()
            }
            x * 2
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Block { stmts, result } => {
            assert_eq!(stmts.len(), 1);
            assert!(matches!(
                stmts[0].kind,
                CstStmtKind::Expr(CstExpr {
                    kind: CstExprKind::If { .. },
                    ..
                })
            ));
            assert!(result.is_some());
        }
        _ => panic!("Expected block"),
    }
}

#[test]
fn test_parse_block_bodied_expr_in_final_position_is_the_result() {
    // In final position the same expression is the block's result, not a
    // statement.
    let source = r"
        {
            if (x < 0) {
                boom()
            }
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Block { stmts, result } => {
            assert!(stmts.is_empty());
            assert!(matches!(
                result.as_deref(),
                Some(CstExpr {
                    kind: CstExprKind::If { .. },
                    ..
                })
            ));
        }
        _ => panic!("Expected block"),
    }
}

#[test]
fn test_parse_match_in_statement_position() {
    let source = r"
        {
            match x {
                0 => zero(),
                _ => other(),
            }
            done()
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Block { stmts, result } => {
            assert_eq!(stmts.len(), 1);
            assert!(matches!(
                stmts[0].kind,
                CstStmtKind::Expr(CstExpr {
                    kind: CstExprKind::Match { .. },
                    ..
                })
            ));
            assert!(result.is_some());
        }
        _ => panic!("Expected block"),
    }
}

#[test]
fn test_parse_non_block_expr_still_requires_semicolon() {
    // The relaxation is scoped to block-bodied expressions: a plain call
    // in statement position still needs its `;`.
    let source = r"
        {
            boom()
            x * 2
        }
    ";
    let mut parser = Parser::new(source).unwrap();
    assert!(parser.parse_expression().is_err());
}

#[test]
fn test_parse_handler_literal() {
    let source = r#"
        {
            FileSystem::read(path) => resume("mock content"),
            FileSystem::write(path, content) => resume(())
        }
    "#;
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::HandlerLiteral(handler_lit) => {
            assert_eq!(handler_lit.methods.len(), 2);

            // Check first method (qualified `FileSystem::read`)
            assert_eq!(
                &*handler_lit.methods[0].ability.segments[0].name,
                "FileSystem"
            );
            assert_eq!(&*handler_lit.methods[0].method.name, "read");
            assert_eq!(handler_lit.methods[0].params.len(), 1);
            assert_eq!(&*handler_lit.methods[0].params[0].name.name, "path");

            // Check second method
            assert_eq!(
                &*handler_lit.methods[1].ability.segments[0].name,
                "FileSystem"
            );
            assert_eq!(&*handler_lit.methods[1].method.name, "write");
            assert_eq!(handler_lit.methods[1].params.len(), 2);
            assert_eq!(&*handler_lit.methods[1].params[0].name.name, "path");
            assert_eq!(&*handler_lit.methods[1].params[1].name.name, "content");
        }
        _ => panic!("Expected handler literal, got {:?}", expr.kind),
    }
}

#[test]
fn test_parse_handler_literal_single_method() {
    let source = r"{ Stdio::print(msg) => resume(()) }";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::HandlerLiteral(handler_lit) => {
            assert_eq!(handler_lit.methods.len(), 1);
            assert_eq!(&*handler_lit.methods[0].ability.segments[0].name, "Stdio");
            assert_eq!(&*handler_lit.methods[0].method.name, "print");
        }
        _ => panic!("Expected handler literal"),
    }
}

#[test]
fn test_parse_handler_literal_qualified_ability() {
    let source = r"{ core::system::Clock::now() => resume(42) }";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::HandlerLiteral(handler_lit) => {
            assert_eq!(handler_lit.methods.len(), 1);
            let ability = &handler_lit.methods[0].ability;
            let path: Vec<&str> = ability.segments.iter().map(|s| s.name.as_ref()).collect();
            assert_eq!(path, vec!["core", "system", "Clock"]);
            assert_eq!(&*handler_lit.methods[0].method.name, "now");
            assert!(handler_lit.methods[0].params.is_empty());
        }
        _ => panic!("Expected handler literal"),
    }
}

#[test]
fn test_parse_with_handle_expr() {
    // `with { arms } handle BODY else E`
    let source = r"with { Exception::throw(e) => 0 } handle risky() else double(r)";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Handle(handle) => {
            assert_eq!(handle.handlers.len(), 1);
            assert!(matches!(
                handle.handlers[0].kind,
                CstExprKind::HandlerLiteral(_)
            ));
            assert!(matches!(handle.body.kind, CstExprKind::Call { .. }));
            assert!(handle.else_clause.is_some());
        }
        _ => panic!("Expected handle expression, got {:?}", expr.kind),
    }
}

#[test]
fn test_parse_with_handle_multiple_handlers() {
    // `with v1, v2, { arms } handle BODY`
    let source = r"with mock_fs, mock_net, { Exception::throw(e) => resume(e) } handle unit_test()";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Handle(handle) => {
            assert_eq!(handle.handlers.len(), 3);
            assert!(handle.else_clause.is_none());
        }
        _ => panic!("Expected handle expression, got {:?}", expr.kind),
    }
}

#[test]
fn test_parse_sandbox_with_abilities() {
    let source = r"sandbox with Log, Console { untrusted_code() }";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Sandbox(sandbox) => {
            assert_eq!(sandbox.allowed_abilities.len(), 2);
            assert_eq!(&*sandbox.allowed_abilities[0].segments[0].name, "Log");
            assert_eq!(&*sandbox.allowed_abilities[1].segments[0].name, "Console");
        }
        _ => panic!("Expected sandbox expression"),
    }
}

#[test]
fn test_parse_sandbox_pure() {
    let source = r"sandbox { pure_computation() }";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Sandbox(sandbox) => {
            assert!(sandbox.allowed_abilities.is_empty());
        }
        _ => panic!("Expected sandbox expression"),
    }
}

#[test]
fn test_parse_sandbox_single_ability() {
    let source = r"sandbox with Log { plugin() }";
    let mut parser = Parser::new(source).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Sandbox(sandbox) => {
            assert_eq!(sandbox.allowed_abilities.len(), 1);
            assert_eq!(&*sandbox.allowed_abilities[0].segments[0].name, "Log");
        }
        _ => panic!("Expected sandbox expression"),
    }
}

#[test]
fn test_parse_bare_method_perform() {
    // A bare-method perform: the ability is unspelled (empty segments),
    // implied by an imported ability method.
    let mut parser = Parser::new(r#"seed!("hello")"#).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Perform {
            ability,
            method,
            args,
        } => {
            assert!(ability.segments.is_empty());
            assert_eq!(&*method.name, "seed");
            assert_eq!(args.len(), 1);
        }
        other => panic!("Expected perform, got {other:?}"),
    }
}

#[test]
fn test_parse_bare_method_perform_requires_argument_list() {
    // `seed!` without `(…)` is not a perform (a bare `!` on a value is
    // reserved for future suspended-ability syntax).
    let mut parser = Parser::new("seed! + 1").unwrap();
    assert!(parser.parse_expression().is_err());
}

#[test]
fn test_parse_qualified_perform_still_carries_its_ability() {
    let mut parser = Parser::new(r#"core::system::Stdio::out!("hi")"#).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::Perform {
            ability, method, ..
        } => {
            let path: Vec<&str> = ability.segments.iter().map(|s| s.name.as_ref()).collect();
            assert_eq!(path, ["core", "system", "Stdio"]);
            assert_eq!(&*method.name, "out");
        }
        other => panic!("Expected perform, got {other:?}"),
    }
}

#[test]
fn test_repl_input_binding_is_ident_equals_expr() {
    let mut parser = Parser::new("x = 1 + 2").unwrap();
    match parser.parse_repl_input().expect("parse error") {
        crate::cst::CstReplInput::Binding { name, expr } => {
            assert_eq!(&*name.name, "x");
            assert!(matches!(expr.kind, CstExprKind::Binary { .. }));
        }
        other => panic!("Expected binding, got {other:?}"),
    }
}

#[test]
fn test_repl_input_equality_is_an_expression_not_a_binding() {
    let mut parser = Parser::new("x == 1").unwrap();
    assert!(matches!(
        parser.parse_repl_input().expect("parse error"),
        crate::cst::CstReplInput::Expr(_)
    ));
}

#[test]
fn test_repl_input_bare_ident_and_call_stay_expressions() {
    for source in ["x", "f(x)", "x.method()"] {
        let mut parser = Parser::new(source).unwrap();
        assert!(
            matches!(
                parser.parse_repl_input().expect("parse error"),
                crate::cst::CstReplInput::Expr(_)
            ),
            "{source} should parse as an expression"
        );
    }
}

#[test]
fn test_repl_binding_rejects_trailing_garbage() {
    let mut parser = Parser::new("x = 1 2").unwrap();
    assert!(parser.parse_repl_input().is_err());
}

#[test]
fn test_repl_binding_to_lambda_and_struct_literal() {
    for source in ["f = (x) => x + 1", "p = Point { x: 1, y: 2 }"] {
        let mut parser = Parser::new(source).unwrap();
        assert!(
            matches!(
                parser.parse_repl_input().expect("parse error"),
                crate::cst::CstReplInput::Binding { .. }
            ),
            "{source} should parse as a binding"
        );
    }
}

/// Segment names of a qualified-name expression parsed from `src`.
fn qualified_expr_segments(src: &str) -> Vec<String> {
    let mut parser = Parser::new(src).unwrap();
    let expr = parser.parse_expression().expect("parse error");
    match expr.kind {
        CstExprKind::QualifiedName(qn) => qn.segments.iter().map(|s| s.name.to_string()).collect(),
        other => panic!("expected qualified name, got {other:?}"),
    }
}

#[test]
fn test_parse_workspace_rooted_expression_path() {
    // A leading `::` roots the path at the workspace; it survives as an
    // empty head segment so resolve can key on it.
    assert_eq!(
        qualified_expr_segments("::other_pkg::utils::helper"),
        ["", "other_pkg", "utils", "helper"]
    );
    assert_eq!(
        qualified_expr_segments("::other_pkg::item"),
        ["", "other_pkg", "item"]
    );
}

#[test]
fn test_workspace_rooted_perform_carries_its_ability() {
    let mut parser = Parser::new("::other_pkg::Logger::log!(1)").unwrap();
    let expr = parser.parse_expression().expect("parse error");
    let CstExprKind::Perform {
        ability, method, ..
    } = expr.kind
    else {
        panic!("expected perform, got {:?}", expr.kind);
    };
    let segments: Vec<String> = ability
        .segments
        .iter()
        .map(|s| s.name.to_string())
        .collect();
    assert_eq!(segments, ["", "other_pkg", "Logger"]);
    assert_eq!(method.name.as_ref(), "log");
}

#[test]
fn test_workspace_rooted_head_must_be_a_plain_identifier() {
    // The segment after a leading `::` names a package; prefix keywords
    // (`pkg`, `core`, `super`, `self`) are not package names.
    for src in [
        "::pkg::item",
        "::core::item",
        "::super::item",
        "::self::item",
    ] {
        let mut parser = Parser::new(src).unwrap();
        assert!(
            parser.parse_expression().is_err(),
            "{src} should be rejected"
        );
    }
}
