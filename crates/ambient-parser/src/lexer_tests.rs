use super::*;

fn lex(source: &str) -> Vec<TokenKind> {
    let mut lexer = Lexer::new(source);
    lexer
        .tokenize()
        .expect("lexer error")
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .map(|t| t.kind)
        .collect()
}

fn lex_all(source: &str) -> Vec<TokenKind> {
    let mut lexer = Lexer::new(source);
    lexer
        .tokenize()
        .expect("lexer error")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn test_keywords() {
    assert_eq!(lex("fn"), vec![TokenKind::Fn, TokenKind::Eof]);
    assert_eq!(lex("pub"), vec![TokenKind::Pub, TokenKind::Eof]);
    assert_eq!(lex("let"), vec![TokenKind::Let, TokenKind::Eof]);
    assert_eq!(lex("const"), vec![TokenKind::Const, TokenKind::Eof]);
    assert_eq!(lex("if"), vec![TokenKind::If, TokenKind::Eof]);
    assert_eq!(lex("else"), vec![TokenKind::Else, TokenKind::Eof]);
    assert_eq!(lex("match"), vec![TokenKind::Match, TokenKind::Eof]);
    assert_eq!(lex("true"), vec![TokenKind::True, TokenKind::Eof]);
    assert_eq!(lex("false"), vec![TokenKind::False, TokenKind::Eof]);
    assert_eq!(lex("enum"), vec![TokenKind::Enum, TokenKind::Eof]);
    assert_eq!(lex("type"), vec![TokenKind::Type, TokenKind::Eof]);
    assert_eq!(lex("struct"), vec![TokenKind::Struct, TokenKind::Eof]);
    assert_eq!(lex("ability"), vec![TokenKind::Ability, TokenKind::Eof]);
    assert_eq!(lex("use"), vec![TokenKind::Use, TokenKind::Eof]);
    assert_eq!(lex("with"), vec![TokenKind::With, TokenKind::Eof]);
    assert_eq!(lex("handle"), vec![TokenKind::Handle, TokenKind::Eof]);
    assert_eq!(lex("resume"), vec![TokenKind::Resume, TokenKind::Eof]);
    assert_eq!(lex("return"), vec![TokenKind::Return, TokenKind::Eof]);
    assert_eq!(lex("sandbox"), vec![TokenKind::Sandbox, TokenKind::Eof]);
    assert_eq!(lex("unique"), vec![TokenKind::Unique, TokenKind::Eof]);
}

#[test]
fn test_identifiers() {
    assert_eq!(lex("foo"), vec![TokenKind::Ident, TokenKind::Eof]);
    assert_eq!(lex("foo_bar"), vec![TokenKind::Ident, TokenKind::Eof]);
    assert_eq!(lex("FooBar"), vec![TokenKind::Ident, TokenKind::Eof]);
    assert_eq!(lex("foo123"), vec![TokenKind::Ident, TokenKind::Eof]);
    assert_eq!(lex("_foo"), vec![TokenKind::Ident, TokenKind::Eof]);
    assert_eq!(lex("_"), vec![TokenKind::Underscore, TokenKind::Eof]);
}

#[test]
fn test_numbers() {
    assert_eq!(lex("42"), vec![TokenKind::Number, TokenKind::Eof]);
    assert_eq!(lex("3.14"), vec![TokenKind::Number, TokenKind::Eof]);
    assert_eq!(lex("1e10"), vec![TokenKind::Number, TokenKind::Eof]);
    assert_eq!(lex("1.5e-3"), vec![TokenKind::Number, TokenKind::Eof]);
    assert_eq!(lex("2.5E+10"), vec![TokenKind::Number, TokenKind::Eof]);
}

#[test]
fn test_number_followed_by_e_identifier() {
    // A bare `e`/`E` with no following digit is not an exponent: the number
    // ends and the `e...` begins an identifier. This is what keeps UUID hex
    // groups like `2eb9553c` (from `unique(...)`) from being shredded into a
    // failed scientific-notation literal.
    assert_eq!(
        lex("2eb9553c"),
        vec![TokenKind::Number, TokenKind::Ident, TokenKind::Eof]
    );
    // `e` immediately before a non-digit sign context (e.g. a UUID group
    // boundary `...e-...`) must not error either.
    assert_eq!(
        lex("2eb-1fdf"),
        vec![
            TokenKind::Number,
            TokenKind::Ident,
            TokenKind::Minus,
            TokenKind::Number,
            TokenKind::Ident,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_uuid_literal() {
    // A canonical uppercase UUID is a single token, even though its groups
    // begin with digits and letters that would otherwise start numbers and
    // identifiers.
    assert_eq!(
        lex("A1B2C3D4-0000-0000-0000-000000000001"),
        vec![TokenKind::Uuid, TokenKind::Eof]
    );
    // A leading-digit group whose letters look like an exponent (`2E...`)
    // is still one UUID token, not a shredded number.
    assert_eq!(
        lex("2EB9553C-1FDF-46FB-A8B1-F2C5A1CFCA94"),
        vec![TokenKind::Uuid, TokenKind::Eof]
    );
    // In context: `unique(<uuid>)`.
    assert_eq!(
        lex("unique(A1B2C3D4-0000-0000-0000-000000000001)"),
        vec![
            TokenKind::Unique,
            TokenKind::LParen,
            TokenKind::Uuid,
            TokenKind::RParen,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_lowercase_uuid_is_not_a_uuid_token() {
    // Lowercase hex is not a UUID literal: it falls back to the ordinary
    // number/identifier/minus tokenization, which the parser rejects as a
    // missing UUID rather than silently accepting.
    assert_eq!(
        lex("2eb9553c-1fdf-46fb-a8b1-f2c5a1cfca94"),
        vec![
            TokenKind::Number,
            TokenKind::Ident,
            TokenKind::Minus,
            TokenKind::Number,
            TokenKind::Ident,
            TokenKind::Minus,
            TokenKind::Number,
            TokenKind::Ident,
            TokenKind::Minus,
            TokenKind::Ident,
            TokenKind::Minus,
            TokenKind::Ident,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_uuid_shape_boundaries() {
    // Eight uppercase hex digits with no following dash-group is just an
    // identifier, not a UUID.
    assert_eq!(lex("ABCDEF12"), vec![TokenKind::Ident, TokenKind::Eof]);
    // A trailing hex character past the 12-digit final group means it is a
    // longer (malformed) token, not a UUID; it must not munch as one.
    assert_ne!(
        lex("A1B2C3D4-0000-0000-0000-000000000001A"),
        vec![TokenKind::Uuid, TokenKind::Eof]
    );
}

#[test]
fn test_strings() {
    assert_eq!(lex(r#""hello""#), vec![TokenKind::String, TokenKind::Eof]);
    assert_eq!(
        lex(r#""hello\nworld""#),
        vec![TokenKind::String, TokenKind::Eof]
    );
    assert_eq!(
        lex(r#""escaped \"quote\"""#),
        vec![TokenKind::String, TokenKind::Eof]
    );
}

#[test]
fn test_string_interpolation() {
    // "Hello, ${name}!"
    let tokens = lex(r#""Hello, ${name}!""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::Ident,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );

    // "a${x}b${y}c"
    let tokens = lex(r#""a${x}b${y}c""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::Ident,
            TokenKind::StringMiddle,
            TokenKind::Ident,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_interpolation_with_braced_block() {
    // "x ${ { 1 } }" — a bare block inside interpolation. The block's
    // closing brace must not truncate the interpolation.
    let tokens = lex(r#""x ${ { 1 } }""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::LBrace,
            TokenKind::Number,
            TokenKind::RBrace,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_interpolation_inner_rbrace_span_and_text() {
    // The RBrace that closes a nested block inside interpolation must have
    // a one-character span pointing at the `}`, with matching text.
    let src = r#""x ${ { 1 } }""#;
    let mut lexer = Lexer::new(src);
    let tokens: Vec<Token> = lexer
        .tokenize()
        .expect("lexer error")
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .collect();
    let rbrace = tokens
        .iter()
        .find(|t| t.kind == TokenKind::RBrace)
        .expect("expected an RBrace token");
    assert_eq!(rbrace.text, "}");
    assert_eq!(rbrace.span.end - rbrace.span.start, 1);
    assert_eq!(
        &src[rbrace.span.start as usize..rbrace.span.end as usize],
        "}"
    );
}

#[test]
fn test_interpolation_lambda_block_with_trailing_tokens() {
    // The repro shape: a lambda whose body is a block, with tokens after
    // the inner block still inside the interpolation.
    let tokens = lex(r#""got ${ f((() => { g(); 2 }, 40)) }""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::Ident, // f
            TokenKind::LParen,
            TokenKind::LParen,
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::FatArrow,
            TokenKind::LBrace,
            TokenKind::Ident, // g
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::Semi,
            TokenKind::Number, // 2
            TokenKind::RBrace,
            TokenKind::Comma,
            TokenKind::Number, // 40
            TokenKind::RParen,
            TokenKind::RParen,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_interpolation_with_nested_plain_string() {
    // "a ${ f("hi") } b" — a plain string literal inside interpolation.
    let tokens = lex(r#""a ${ f("hi") } b""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::Ident, // f
            TokenKind::LParen,
            TokenKind::String, // "hi"
            TokenKind::RParen,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_interpolation_with_nested_interpolated_string() {
    // "a ${ "x${y}z" } b" — an interpolated string nested inside an
    // interpolation. The depth stack handles this without a `{`-block.
    let tokens = lex(r#""a ${ "x${y}z" } b""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart, // outer "a ${
            TokenKind::StringStart, // inner "x${
            TokenKind::Ident,       // y
            TokenKind::StringEnd,   // }z"
            TokenKind::StringEnd,   // } b"
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_interpolation_three_levels_of_braces() {
    // "a ${ { { 1 } } } b" — nested blocks, three deep, inside one
    // interpolation.
    let tokens = lex(r#""a ${ { { 1 } } } b""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::LBrace,
            TokenKind::LBrace,
            TokenKind::Number,
            TokenKind::RBrace,
            TokenKind::RBrace,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_operators() {
    assert_eq!(lex("+"), vec![TokenKind::Plus, TokenKind::Eof]);
    assert_eq!(lex("-"), vec![TokenKind::Minus, TokenKind::Eof]);
    assert_eq!(lex("*"), vec![TokenKind::Star, TokenKind::Eof]);
    assert_eq!(lex("/"), vec![TokenKind::Slash, TokenKind::Eof]);
    assert_eq!(lex("%"), vec![TokenKind::Percent, TokenKind::Eof]);
    assert_eq!(lex("=="), vec![TokenKind::EqEq, TokenKind::Eof]);
    assert_eq!(lex("!="), vec![TokenKind::Ne, TokenKind::Eof]);
    assert_eq!(lex("<"), vec![TokenKind::Lt, TokenKind::Eof]);
    assert_eq!(lex("<="), vec![TokenKind::Le, TokenKind::Eof]);
    assert_eq!(lex(">"), vec![TokenKind::Gt, TokenKind::Eof]);
    assert_eq!(lex(">="), vec![TokenKind::Ge, TokenKind::Eof]);
    assert_eq!(lex("&&"), vec![TokenKind::AndAnd, TokenKind::Eof]);
    assert_eq!(lex("||"), vec![TokenKind::OrOr, TokenKind::Eof]);
    assert_eq!(lex("!"), vec![TokenKind::Bang, TokenKind::Eof]);
    assert_eq!(lex("="), vec![TokenKind::Eq, TokenKind::Eof]);
    assert_eq!(lex("=>"), vec![TokenKind::FatArrow, TokenKind::Eof]);
    assert_eq!(lex("->"), vec![TokenKind::Arrow, TokenKind::Eof]);
}

#[test]
fn test_punctuation() {
    assert_eq!(lex("("), vec![TokenKind::LParen, TokenKind::Eof]);
    assert_eq!(lex(")"), vec![TokenKind::RParen, TokenKind::Eof]);
    assert_eq!(lex("{"), vec![TokenKind::LBrace, TokenKind::Eof]);
    assert_eq!(lex("}"), vec![TokenKind::RBrace, TokenKind::Eof]);
    assert_eq!(lex("["), vec![TokenKind::LBracket, TokenKind::Eof]);
    assert_eq!(lex("]"), vec![TokenKind::RBracket, TokenKind::Eof]);
    assert_eq!(lex(","), vec![TokenKind::Comma, TokenKind::Eof]);
    assert_eq!(lex(";"), vec![TokenKind::Semi, TokenKind::Eof]);
    assert_eq!(lex(":"), vec![TokenKind::Colon, TokenKind::Eof]);
    assert_eq!(lex("."), vec![TokenKind::Dot, TokenKind::Eof]);
}

#[test]
fn test_whitespace_preserved() {
    let tokens = lex_all("a b");
    assert_eq!(
        tokens,
        vec![
            TokenKind::Ident,
            TokenKind::Whitespace,
            TokenKind::Ident,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_comments_preserved() {
    let tokens = lex_all("a // comment\nb");
    assert_eq!(
        tokens,
        vec![
            TokenKind::Ident,
            TokenKind::Whitespace,
            TokenKind::Comment,
            TokenKind::Whitespace,
            TokenKind::Ident,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_function_definition() {
    let tokens = lex("fn add(x: number, y: number): number { x + y }");
    assert_eq!(
        tokens,
        vec![
            TokenKind::Fn,
            TokenKind::Ident,
            TokenKind::LParen,
            TokenKind::Ident,
            TokenKind::Colon,
            TokenKind::Ident,
            TokenKind::Comma,
            TokenKind::Ident,
            TokenKind::Colon,
            TokenKind::Ident,
            TokenKind::RParen,
            TokenKind::Colon,
            TokenKind::Ident,
            TokenKind::LBrace,
            TokenKind::Ident,
            TokenKind::Plus,
            TokenKind::Ident,
            TokenKind::RBrace,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_nested_braces_in_interpolation() {
    // "value: ${{ x: 1 }}"
    let tokens = lex(r#""value: ${{ x: 1 }}""#);
    assert_eq!(
        tokens,
        vec![
            TokenKind::StringStart,
            TokenKind::LBrace,
            TokenKind::Ident,
            TokenKind::Colon,
            TokenKind::Number,
            TokenKind::RBrace,
            TokenKind::StringEnd,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_error_unterminated_string() {
    let mut lexer = Lexer::new(r#""hello"#);
    let result = lexer.tokenize();
    assert!(result.is_err());
    assert!(matches!(
        result.expect_err("expected error").kind,
        ParseErrorKind::UnterminatedString
    ));
}

#[test]
fn test_error_invalid_escape() {
    let mut lexer = Lexer::new(r#""hello\x""#);
    let result = lexer.tokenize();
    assert!(result.is_err());
    assert!(matches!(
        result.expect_err("expected error").kind,
        ParseErrorKind::InvalidEscape('x')
    ));
}

// ─────────────────────────────────────────────────────────────────────────────
// tokenize_for_highlighting
// ─────────────────────────────────────────────────────────────────────────────

/// Assert the highlighting invariants: spans are in-order, non-overlapping,
/// on char boundaries, and (with gaps copied verbatim) reconstruct the input.
fn assert_highlight_covers(source: &str) -> Vec<(Span, TokenKind)> {
    let tokens = tokenize_for_highlighting(source);
    let mut reconstructed = String::new();
    let mut cursor = 0usize;
    for &(span, _) in &tokens {
        let (start, end) = (span.start as usize, span.end as usize);
        assert!(start >= cursor, "spans out of order in {source:?}");
        assert!(end >= start, "inverted span in {source:?}");
        assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
        reconstructed.push_str(&source[cursor..start]);
        reconstructed.push_str(&source[start..end]);
        cursor = end;
    }
    reconstructed.push_str(&source[cursor..]);
    assert_eq!(reconstructed, source);
    tokens
}

#[test]
fn test_highlighting_never_fails_on_broken_input() {
    // None of these lex cleanly; all must still produce covering tokens.
    for source in [
        r#""unterminated"#,
        r#"let s = "half ${name"#,
        r#""bad \x escape""#,
        "let a = b & c",
        "let a = b | c",
        "emoji 🦀 outside a string",
        "café",
        "@#$",
        "\"unterminated with unicode é🦀",
    ] {
        assert_highlight_covers(source);
    }
}

#[test]
fn test_highlighting_unterminated_string_classified_as_string() {
    let tokens = assert_highlight_covers(r#"let x = "hel"#);
    let (span, kind) = *tokens.last().expect("tokens");
    assert_eq!(kind, TokenKind::String);
    assert_eq!(span, Span::new(8, 12));
}

#[test]
fn test_highlighting_stray_char_is_error_token() {
    let tokens = assert_highlight_covers("1 @ 2");
    let kinds: Vec<TokenKind> = tokens.iter().map(|&(_, k)| k).collect();
    assert_eq!(
        kinds,
        [
            TokenKind::Number,
            TokenKind::Whitespace,
            TokenKind::Error,
            TokenKind::Whitespace,
            TokenKind::Number,
        ]
    );
}

#[test]
fn test_highlighting_resumes_after_error() {
    // The stray `&` errors, but the keyword after it is still classified.
    let tokens = assert_highlight_covers("a & fn");
    let kinds: Vec<TokenKind> = tokens.iter().map(|&(_, k)| k).collect();
    assert!(kinds.contains(&TokenKind::Error));
    assert!(kinds.contains(&TokenKind::Fn));
}

#[test]
fn test_highlighting_matches_lexer_on_valid_input() {
    let source = r#"pub fn greet(name: String): String { "hi ${name}" } // done"#;
    let expected: Vec<(Span, TokenKind)> = Lexer::new(source)
        .tokenize()
        .expect("valid source")
        .into_iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| (t.span, t.kind))
        .collect();
    assert_eq!(assert_highlight_covers(source), expected);
}
