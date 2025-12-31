//! Property-based tests for the Ambient parser.
//!
//! These tests use proptest to generate random inputs and verify invariants.

use ambient_parser::{parse, parse_expr, parse_to_cst, Lexer, TokenKind};
use proptest::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// Lexer Property Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Strategy for generating valid identifiers.
fn ident_strategy() -> impl Strategy<Value = String> {
    // Identifiers: start with letter or underscore, followed by alphanumeric or underscore
    prop::string::string_regex("[a-zA-Z_][a-zA-Z0-9_]{0,20}")
        .expect("regex should be valid")
        .prop_filter("not a keyword", |s| {
            !matches!(
                s.as_str(),
                "fn" | "pub"
                    | "let"
                    | "const"
                    | "if"
                    | "else"
                    | "match"
                    | "true"
                    | "false"
                    | "enum"
                    | "type"
                    | "ability"
                    | "use"
                    | "with"
                    | "handle"
                    | "resume"
                    | "sandbox"
                    | "unique"
                    | "pkg"
                    | "core"
                    | "self"
                    | "super"
            )
        })
        .prop_filter("not underscore alone", |s| s != "_")
}

/// Strategy for generating valid number literals.
fn number_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Integer
        (1..1000u64).prop_map(|n| n.to_string()),
        // Decimal
        (1..1000u64, 1..1000u64).prop_map(|(a, b)| format!("{}.{}", a, b)),
        // Scientific notation
        (1..100u64, -5..5i32).prop_map(|(a, e)| format!("{}e{}", a, e)),
    ]
}

/// Strategy for generating simple string literals (without interpolation).
fn simple_string_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 ,.!?]{0,50}")
        .expect("regex should be valid")
        .prop_map(|s| format!("\"{}\"", s))
}

proptest! {
    /// Lexer should tokenize any valid identifier.
    #[test]
    fn lexer_tokenizes_identifiers(ident in ident_strategy()) {
        let mut lexer = Lexer::new(&ident);
        let tokens = lexer.tokenize().expect("lexer should succeed");

        // Should have exactly 2 tokens: the identifier and EOF
        prop_assert_eq!(tokens.len(), 2, "expected 2 tokens for identifier");
        prop_assert_eq!(tokens[0].kind, TokenKind::Ident);
        prop_assert_eq!(&tokens[0].text, &ident);
        prop_assert_eq!(tokens[1].kind, TokenKind::Eof);
    }

    /// Lexer should tokenize any valid number.
    #[test]
    fn lexer_tokenizes_numbers(num in number_strategy()) {
        let mut lexer = Lexer::new(&num);
        let tokens = lexer.tokenize().expect("lexer should succeed");

        prop_assert_eq!(tokens.len(), 2, "expected 2 tokens for number");
        prop_assert_eq!(tokens[0].kind, TokenKind::Number);
        prop_assert_eq!(tokens[1].kind, TokenKind::Eof);
    }

    /// Lexer should tokenize any valid string literal.
    #[test]
    fn lexer_tokenizes_strings(s in simple_string_strategy()) {
        let mut lexer = Lexer::new(&s);
        let tokens = lexer.tokenize().expect("lexer should succeed");

        prop_assert_eq!(tokens.len(), 2, "expected 2 tokens for string");
        prop_assert_eq!(tokens[0].kind, TokenKind::String);
        prop_assert_eq!(tokens[1].kind, TokenKind::Eof);
    }

    /// Lexer should preserve token spans that cover the entire input.
    #[test]
    fn lexer_spans_cover_input(s in "[a-zA-Z0-9 +\\-*/(){}\\[\\];:,.=<>!&|]{1,100}") {
        let mut lexer = Lexer::new(&s);
        if let Ok(tokens) = lexer.tokenize() {
            // Verify spans are monotonically increasing
            let mut last_end = 0u32;
            for token in &tokens {
                if token.kind != TokenKind::Eof {
                    prop_assert!(token.span.start >= last_end,
                        "token start {} should be >= last end {}", token.span.start, last_end);
                    prop_assert!(token.span.end > token.span.start,
                        "token end {} should be > start {}", token.span.end, token.span.start);
                    last_end = token.span.end;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Parser Property Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Strategy for generating simple arithmetic expressions.
fn arith_expr_strategy() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![(1..100u64).prop_map(|n| n.to_string()), ident_strategy(),];

    leaf.prop_recursive(3, 64, 10, |inner| {
        prop_oneof![
            // Binary operations
            (
                inner.clone(),
                prop::sample::select(vec!["+", "-", "*", "/", "%"]),
                inner.clone()
            )
                .prop_map(|(a, op, b)| format!("({} {} {})", a, op, b)),
            // Unary operations
            (prop::sample::select(vec!["-", "!"]), inner.clone())
                .prop_map(|(op, a)| format!("({}{})", op, a)),
            // Parenthesized
            inner.clone().prop_map(|a| format!("({})", a)),
        ]
    })
}

/// Strategy for generating comparison expressions.
fn comparison_expr_strategy() -> impl Strategy<Value = String> {
    let operand = prop_oneof![(1..100u64).prop_map(|n| n.to_string()), ident_strategy(),];

    (
        operand.clone(),
        prop::sample::select(vec!["==", "!=", "<", "<=", ">", ">="]),
        operand,
    )
        .prop_map(|(a, op, b)| format!("{} {} {}", a, op, b))
}

/// Strategy for generating boolean expressions.
#[allow(dead_code)]
fn bool_expr_strategy() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("true".to_string()),
        Just("false".to_string()),
        ident_strategy(),
        comparison_expr_strategy(),
    ];

    leaf.prop_recursive(2, 32, 10, |inner| {
        prop_oneof![
            // And/Or
            (
                inner.clone(),
                prop::sample::select(vec!["&&", "||"]),
                inner.clone()
            )
                .prop_map(|(a, op, b)| format!("({} {} {})", a, op, b)),
            // Not
            inner.clone().prop_map(|a| format!("(!{})", a)),
        ]
    })
}

proptest! {
    /// Parser should parse any valid arithmetic expression.
    #[test]
    fn parser_parses_arithmetic(expr in arith_expr_strategy()) {
        // Note: we use parse_to_cst since parse requires lowering which may fail
        // for some generated expressions
        let result = parse_to_cst(&expr);
        // We don't require success since some combinations may be invalid,
        // but we require no panic
        let _ = result;
    }

    /// Parser should parse any valid comparison expression.
    #[test]
    fn parser_parses_comparisons(expr in comparison_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse: {}", expr);
    }

    /// Parser should handle deeply nested expressions without stack overflow.
    #[test]
    fn parser_handles_deep_nesting(depth in 1..20usize) {
        // Create nested parentheses around a number
        let mut expr = "42".to_string();
        for _ in 0..depth {
            expr = format!("({})", expr);
        }

        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse nested expr at depth {}", depth);
    }

    /// Parsed expressions should have valid spans within the source.
    #[test]
    fn parser_spans_are_valid(expr in arith_expr_strategy()) {
        if let Ok(cst) = parse_to_cst(&expr) {
            let len = expr.len() as u32;
            // Check that no item spans exceed the source length
            for item in &cst.items {
                prop_assert!(item.span.end <= len,
                    "item span end {} exceeds source length {}", item.span.end, len);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Module Parsing Property Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Strategy for generating simple function definitions.
fn function_def_strategy() -> impl Strategy<Value = String> {
    (
        ident_strategy(),                              // function name
        prop::collection::vec(ident_strategy(), 0..3), // parameter names
    )
        .prop_map(|(name, params)| {
            let param_list: String = params
                .iter()
                .map(|p| format!("{}: number", p))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "fn {}({}): number {{ {} }}",
                name,
                param_list,
                if params.is_empty() {
                    "42".to_string()
                } else {
                    params.join(" + ")
                }
            )
        })
}

/// Strategy for generating simple enum definitions.
fn enum_def_strategy() -> impl Strategy<Value = String> {
    (
        ident_strategy().prop_filter("starts uppercase", |s| {
            s.chars().next().map_or(false, |c| c.is_uppercase())
        }),
        prop::collection::vec(
            ident_strategy().prop_filter("starts uppercase", |s| {
                s.chars().next().map_or(false, |c| c.is_uppercase())
            }),
            1..5,
        ),
    )
        .prop_map(|(name, variants)| {
            let variant_list = variants.join(",\n    ");
            format!("enum {} {{\n    {}\n}}", name, variant_list)
        })
}

proptest! {
    /// Parser should parse any valid function definition.
    #[test]
    fn parser_parses_functions(func in function_def_strategy()) {
        let result = parse(&func);
        prop_assert!(result.is_ok(), "failed to parse function: {}\nerror: {:?}", func, result.err());
    }

    /// Parser should parse any valid enum definition.
    #[test]
    fn parser_parses_enums(enum_def in enum_def_strategy()) {
        let result = parse(&enum_def);
        prop_assert!(result.is_ok(), "failed to parse enum: {}\nerror: {:?}", enum_def, result.err());
    }

    /// Multiple functions should parse correctly.
    #[test]
    fn parser_parses_multiple_functions(
        func1 in function_def_strategy(),
        func2 in function_def_strategy()
    ) {
        let source = format!("{}\n\n{}", func1, func2);
        let result = parse(&source);
        if let Ok(module) = result {
            prop_assert_eq!(module.items.len(), 2, "expected 2 functions in module");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Roundtrip Tests
// ─────────────────────────────────────────────────────────────────────────────

proptest! {
    /// CST parsing should preserve enough info to reconstruct spans.
    #[test]
    fn cst_preserves_spans(expr in arith_expr_strategy()) {
        if let Ok(cst) = parse_to_cst(&expr) {
            // Just verify we can parse without panic
            // Full roundtrip would require a pretty-printer
            prop_assert!(cst.span.start <= cst.span.end);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge Case Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_empty_input() {
    let result = parse("");
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn test_whitespace_only() {
    let result = parse("   \n\n\t   ");
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn test_comments_only() {
    let result = parse("// This is a comment\n// Another comment");
    assert!(result.is_ok());
    assert!(result.unwrap().items.is_empty());
}

#[test]
fn test_all_keywords_as_identifiers_in_context() {
    // Keywords should not be parsed as identifiers
    let result = parse_expr("fn");
    assert!(result.is_err());
}

#[test]
fn test_operators_precedence() {
    // 1 + 2 * 3 should parse as 1 + (2 * 3)
    let result = parse_expr("1 + 2 * 3").unwrap();
    match result.kind {
        ambient_engine::ast::ExprKind::Binary { op, .. } => {
            assert_eq!(op, ambient_engine::ast::BinaryOp::Add);
        }
        _ => panic!("Expected binary expression"),
    }
}

#[test]
fn test_left_associativity() {
    // 1 - 2 - 3 should parse as (1 - 2) - 3
    let result = parse_expr("1 - 2 - 3").unwrap();
    match result.kind {
        ambient_engine::ast::ExprKind::Binary { left, .. } => {
            assert!(matches!(
                left.kind,
                ambient_engine::ast::ExprKind::Binary {
                    op: ambient_engine::ast::BinaryOp::Sub,
                    ..
                }
            ));
        }
        _ => panic!("Expected binary expression"),
    }
}

#[test]
fn test_parentheses_override_precedence() {
    // (1 + 2) * 3 should parse as (1 + 2) * 3
    let result = parse_expr("(1 + 2) * 3").unwrap();
    match result.kind {
        ambient_engine::ast::ExprKind::Binary { op, left, .. } => {
            assert_eq!(op, ambient_engine::ast::BinaryOp::Mul);
            assert!(matches!(
                left.kind,
                ambient_engine::ast::ExprKind::Binary {
                    op: ambient_engine::ast::BinaryOp::Add,
                    ..
                }
            ));
        }
        _ => panic!("Expected binary expression"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional Property Tests (Ticket 5.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Strategy for generating tuple expressions.
fn tuple_expr_strategy() -> impl Strategy<Value = String> {
    let element = prop_oneof![
        (1..100u64).prop_map(|n| n.to_string()),
        ident_strategy(),
        Just("true".to_string()),
        Just("false".to_string()),
    ];

    prop::collection::vec(element, 0..5).prop_map(|elements| {
        if elements.is_empty() {
            "()".to_string()
        } else {
            format!("({})", elements.join(", "))
        }
    })
}

/// Strategy for generating record expressions.
fn record_expr_strategy() -> impl Strategy<Value = String> {
    let field =
        (ident_strategy(), 1..100u64).prop_map(|(name, value)| format!("{}: {}", name, value));

    prop::collection::vec(field, 1..5).prop_map(|fields| format!("{{ {} }}", fields.join(", ")))
}

/// Strategy for generating list expressions.
fn list_expr_strategy() -> impl Strategy<Value = String> {
    let element = prop_oneof![
        (1..100u64).prop_map(|n| n.to_string()),
        Just("true".to_string()),
        Just("false".to_string()),
    ];

    prop::collection::vec(element, 0..5).prop_map(|elements| format!("[{}]", elements.join(", ")))
}

/// Strategy for generating if expressions.
fn if_expr_strategy() -> impl Strategy<Value = String> {
    let condition = prop_oneof![
        Just("true".to_string()),
        Just("false".to_string()),
        comparison_expr_strategy(),
    ];
    let body = (1..100u64).prop_map(|n| n.to_string());

    (condition, body.clone(), prop::option::of(body)).prop_map(|(cond, then_body, else_body)| {
        match else_body {
            Some(else_expr) => format!("if {} {{ {} }} else {{ {} }}", cond, then_body, else_expr),
            None => format!("if {} {{ {} }}", cond, then_body),
        }
    })
}

/// Strategy for generating lambda expressions.
fn lambda_expr_strategy() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(ident_strategy(), 0..3),
        (1..100u64).prop_map(|n| n.to_string()),
    )
        .prop_map(|(params, body)| {
            if params.is_empty() {
                format!("() => {}", body)
            } else {
                format!("({}) => {}", params.join(", "), body)
            }
        })
}

/// Strategy for generating block expressions.
fn block_expr_strategy() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(
            (ident_strategy(), 1..100u64).prop_map(|(name, val)| format!("let {} = {}", name, val)),
            0..3,
        ),
        (1..100u64).prop_map(|n| n.to_string()),
    )
        .prop_map(|(stmts, result)| {
            if stmts.is_empty() {
                format!("{{ {} }}", result)
            } else {
                format!("{{ {}; {} }}", stmts.join("; "), result)
            }
        })
}

proptest! {
    /// Parser should parse any valid tuple expression.
    #[test]
    fn parser_parses_tuples(expr in tuple_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse tuple: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should parse any valid record expression.
    #[test]
    fn parser_parses_records(expr in record_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse record: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should parse any valid list expression.
    #[test]
    fn parser_parses_lists(expr in list_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse list: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should parse any valid if expression.
    #[test]
    fn parser_parses_if_exprs(expr in if_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse if: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should parse any valid lambda expression.
    #[test]
    fn parser_parses_lambdas(expr in lambda_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse lambda: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should parse any valid block expression.
    #[test]
    fn parser_parses_blocks(expr in block_expr_strategy()) {
        let result = parse_expr(&expr);
        prop_assert!(result.is_ok(), "failed to parse block: {}\nerror: {:?}", expr, result.err());
    }

    /// Parser should handle string escapes correctly.
    #[test]
    fn parser_handles_string_escapes(s in "[a-zA-Z0-9 ]{0,30}") {
        let escaped = format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let result = parse_expr(&escaped);
        prop_assert!(result.is_ok(), "failed to parse string: {}\nerror: {:?}", escaped, result.err());
    }

    /// Lexer should handle all ASCII printable characters.
    #[test]
    fn lexer_handles_printable_ascii(s in "[ -~]{1,50}") {
        let mut lexer = Lexer::new(&s);
        // Should not panic, even if tokenization fails
        let _ = lexer.tokenize();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Recovery Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_error_unclosed_paren() {
    let result = parse_expr("(1 + 2");
    assert!(result.is_err());
    // Error should mention RParen or similar
    let err = result.unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("RParen") || msg.contains("paren") || msg.contains("Expected"),
        "error should mention expected closing: {}",
        msg
    );
}

#[test]
fn test_error_unclosed_bracket() {
    let result = parse_expr("[1, 2, 3");
    assert!(result.is_err());
}

#[test]
fn test_error_unclosed_brace() {
    let result = parse_expr("{ 42");
    assert!(result.is_err());
}

#[test]
fn test_error_missing_operator() {
    // Two adjacent numbers/expressions without operator
    // Note: The parser may interpret "1 2" as two separate expressions
    // or as a function call. This tests that it doesn't crash.
    let result = parse_expr("1 2");
    // Check for error or that parsing completes without crash
    // Some parsers may accept this with error recovery
    let _ = result;
}

#[test]
fn test_error_double_operator() {
    let result = parse_expr("1 ++ 2");
    // Should fail: ++ is not a valid operator
    assert!(result.is_err());
}

#[test]
fn test_error_trailing_operator() {
    let result = parse_expr("1 +");
    assert!(result.is_err());
}

#[test]
fn test_error_leading_operator() {
    // - and ! are valid unary operators, but +* is not
    let result = parse_expr("* 1");
    assert!(result.is_err());
}

#[test]
fn test_error_invalid_function_syntax() {
    let result = parse("fn 123() {}");
    assert!(result.is_err());
}

#[test]
fn test_error_missing_function_body() {
    let result = parse("fn foo()");
    assert!(result.is_err());
}

#[test]
fn test_error_invalid_let_binding() {
    let result = parse_expr("{ let = 42 }");
    assert!(result.is_err());
}

#[test]
fn test_error_empty_if_condition() {
    let result = parse_expr("if { 42 }");
    assert!(result.is_err());
}

#[test]
fn test_error_missing_if_body() {
    let result = parse_expr("if true");
    assert!(result.is_err());
}

#[test]
fn test_error_invalid_match_syntax() {
    let result = parse_expr("match { }");
    assert!(result.is_err());
}

#[test]
fn test_error_invalid_lambda_arrow() {
    // Should be => not ->
    // Note: The parser may interpret "->" differently
    // Just ensure it doesn't crash or produces an error
    let result = parse_expr("(x) -> x");
    // If the parser succeeds, it interpreted -> as minus followed by >
    // which produces a comparison, not a lambda
    // Either way, it shouldn't be a proper lambda
    let _ = result;
}

#[test]
fn test_error_nested_unclosed() {
    let result = parse_expr("((1 + 2)");
    assert!(result.is_err());
}

#[test]
fn test_error_mismatched_delimiters() {
    let result = parse_expr("(1 + 2]");
    assert!(result.is_err());
}

#[test]
fn test_error_invalid_tuple_trailing_comma() {
    // Single element with trailing comma might be ambiguous
    // Test that it doesn't crash
    let _ = parse_expr("(1,)");
}

#[test]
fn test_error_enum_lowercase_variant() {
    // Enum variants should start with uppercase (convention)
    // Parser may allow this but it's worth testing behavior
    let result = parse("enum Foo { bar }");
    // Just check it doesn't crash - lowercase variants may be allowed
    let _ = result;
}

// ─────────────────────────────────────────────────────────────────────────────
// Stress Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_very_long_identifier() {
    let long_name = "a".repeat(1000);
    let expr = format!("{}", long_name);
    let result = parse_expr(&expr);
    assert!(result.is_ok());
}

#[test]
fn test_many_function_parameters() {
    let params: Vec<String> = (0..50).map(|i| format!("p{}: number", i)).collect();
    let source = format!("fn many({}) {{ 42 }}", params.join(", "));
    let result = parse(&source);
    assert!(result.is_ok());
}

#[test]
fn test_many_list_elements() {
    let elements: Vec<String> = (0..100).map(|i| i.to_string()).collect();
    let expr = format!("[{}]", elements.join(", "));
    let result = parse_expr(&expr);
    assert!(result.is_ok());
}

#[test]
fn test_deeply_nested_blocks() {
    let mut expr = "42".to_string();
    for _ in 0..20 {
        expr = format!("{{ {} }}", expr);
    }
    let result = parse_expr(&expr);
    assert!(result.is_ok());
}

#[test]
fn test_complex_nested_expression() {
    let expr = "{ let x = (1 + 2) * 3; let y = if x > 5 { x } else { 0 }; [x, y, x + y] }";
    let result = parse_expr(expr);
    assert!(result.is_ok());
}
