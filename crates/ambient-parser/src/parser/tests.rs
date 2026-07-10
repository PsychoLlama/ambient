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

/// Parse a module expected to contain use items, and flatten them
/// through lowering (the semantic surface tests care about).
fn flatten_uses(source: &str) -> Vec<ambient_engine::ast::UseDef> {
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let lowered = crate::lower::lower_module(&module).expect("lower error");
    lowered
        .items
        .into_iter()
        .filter_map(|item| match item.kind {
            ambient_engine::ast::ItemKind::Use(u) => Some(u),
            _ => None,
        })
        .collect()
}

fn path_names(u: &ambient_engine::ast::UseDef) -> Vec<&str> {
    u.path.iter().map(|(name, _)| name.as_ref()).collect()
}

#[test]
fn test_parse_use_pkg_module() {
    let uses = flatten_uses("use pkg::utils;");
    assert_eq!(uses.len(), 1);
    assert!(!uses[0].is_public);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["utils"]);
    assert!(uses[0].alias.is_none());
}

#[test]
fn test_parse_use_pkg_nested() {
    let uses = flatten_uses("use pkg::utils::format;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["utils", "format"]);
}

#[test]
fn test_parse_use_pkg_items() {
    // Braces are pure grouping: the tree flattens to one UseDef per leaf.
    let uses = flatten_uses("use pkg::utils::{format, parse};");
    assert_eq!(uses.len(), 2);
    assert_eq!(path_names(&uses[0]), ["utils", "format"]);
    assert_eq!(path_names(&uses[1]), ["utils", "parse"]);
}

#[test]
fn test_parse_use_nested_groups() {
    let uses = flatten_uses("use pkg::a::{b::c, d::{e, f as g}};");
    assert_eq!(uses.len(), 3);
    assert_eq!(path_names(&uses[0]), ["a", "b", "c"]);
    assert_eq!(path_names(&uses[1]), ["a", "d", "e"]);
    assert_eq!(path_names(&uses[2]), ["a", "d", "f"]);
    assert_eq!(uses[2].alias.as_ref().map(|(n, _)| n.as_ref()), Some("g"));
    assert_eq!(uses[2].local_name().map(AsRef::as_ref), Some("g"));
}

#[test]
fn test_parse_use_root_group() {
    let uses = flatten_uses("use {core::primitives::number, core::system::Stdio};");
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["primitives", "number"]);
    assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[1]), ["system", "Stdio"]);
}

#[test]
fn test_parse_use_alias() {
    let uses = flatten_uses("use core::primitives::number::sqrt as root2;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["primitives", "number", "sqrt"]);
    assert_eq!(uses[0].local_name().map(AsRef::as_ref), Some("root2"));
}

#[test]
fn test_parse_use_local_root() {
    // A path rooted at a module alias from another use.
    let uses = flatten_uses("use pkg::deep::nested;\nuse nested::leaf::f;");
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Local);
    assert_eq!(path_names(&uses[1]), ["nested", "leaf", "f"]);
}

#[test]
fn test_parse_use_super_chain() {
    let uses = flatten_uses("use super::super::m::f;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
    assert_eq!(path_names(&uses[0]), ["m", "f"]);
}

#[test]
fn test_parse_use_keyword_mid_path_is_error() {
    let source = "use pkg::a::core::b;";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse ok");
    assert!(crate::lower::lower_module(&module).is_err());
}

#[test]
fn test_parse_use_in_block() {
    let source = "fn f(): Number {\n  use core::primitives::number::sqrt;\n  sqrt(16)\n}";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let lowered = crate::lower::lower_module(&module).expect("lower error");
    let ambient_engine::ast::ItemKind::Function(f) = &lowered.items[0].kind else {
        panic!("expected function");
    };
    let ambient_engine::ast::ExprKind::Block(stmts, result) = &f.body.kind else {
        panic!("expected block body");
    };
    assert!(matches!(
        stmts[0].kind,
        ambient_engine::ast::StmtKind::Use(_)
    ));
    assert!(result.is_some());
}

#[test]
fn test_parse_use_core_system() {
    // Platform abilities live under `core::system`, an ordinary `core`
    // path — no dedicated root.
    let uses = flatten_uses("use core::system::Network;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["system", "Network"]);
}

#[test]
fn test_parse_use_pkg_named_platform() {
    // `platform` is now an ordinary identifier: a user path segment
    // `platform` under `pkg` parses as `Pkg`.
    let uses = flatten_uses("use pkg::platform;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["platform"]);
}

#[test]
fn test_parse_use_platform_is_local_alias() {
    // With the reserved `platform` root removed, `use platform::X`
    // parses as an alias-rooted (`Local`) path like any other bare
    // head — it no longer names a reserved root.
    let uses = flatten_uses("use platform::Network;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Local);
    assert_eq!(path_names(&uses[0]), ["platform", "Network"]);
}

#[test]
fn test_parse_use_self() {
    let uses = flatten_uses("use self::sibling;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Self_);
    assert_eq!(path_names(&uses[0]), ["sibling"]);
}

#[test]
fn test_parse_use_super() {
    let uses = flatten_uses("use super::parent;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(1));
    assert_eq!(path_names(&uses[0]), ["parent"]);
}

#[test]
fn test_parse_use_super_super() {
    let uses = flatten_uses("use super::super::grandparent;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
    assert_eq!(path_names(&uses[0]), ["grandparent"]);
}

#[test]
fn test_parse_pub_use() {
    let uses = flatten_uses("pub use pkg::utils;");
    assert_eq!(uses.len(), 1);
    assert!(uses[0].is_public);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
}
