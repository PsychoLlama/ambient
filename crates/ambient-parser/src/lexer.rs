//! Lexer (tokenizer) for the Ambient language.
//!
//! The lexer converts source text into a stream of tokens, handling:
//! - Keywords and identifiers
//! - Number literals (f64)
//! - UUID literals (canonical uppercase `8-4-4-4-12` hex, for `unique(...)`)
//! - String literals with interpolation (`${expr}`)
//! - Comments (line comments `//`)
//! - Operators and punctuation
//!
//! The lexer preserves all whitespace and comments as trivia for CST construction.

use std::iter::Peekable;
use std::str::CharIndices;

use ambient_engine::ast::Span;

use crate::error::{ParseError, ParseErrorKind};

/// A token produced by the lexer.
#[derive(Debug, Clone)]
pub struct Token {
    /// The kind of token.
    pub kind: TokenKind,
    /// Source span.
    pub span: Span,
    /// The source text of the token.
    pub text: String,
}

impl Token {
    /// Create a new token.
    #[must_use]
    pub fn new(kind: TokenKind, span: Span, text: impl Into<String>) -> Self {
        Self {
            kind,
            span,
            text: text.into(),
        }
    }
}

/// The kind of token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // ─────────────────────────────────────────────────────────────────────────
    // Keywords
    // ─────────────────────────────────────────────────────────────────────────
    /// `fn`
    Fn,
    /// `pub`
    Pub,
    /// `let`
    Let,
    /// `const`
    Const,
    /// `if`
    If,
    /// `else`
    Else,
    /// `match`
    Match,
    /// `true`
    True,
    /// `false`
    False,
    /// `enum`
    Enum,
    /// `type`
    Type,
    /// `struct`
    Struct,
    /// `ability`
    Ability,
    /// `use`
    Use,
    /// `with`
    With,
    /// `handle`
    Handle,
    /// `resume`
    Resume,
    /// `sandbox`
    Sandbox,
    /// `unique`
    Unique,
    /// `extern`
    Extern,
    /// `trait`
    Trait,
    /// `impl`
    Impl,
    /// `for`
    For,
    /// `where`
    Where,
    /// `pkg` - local package prefix in imports
    Pkg,
    /// `core` - standard library prefix in imports
    Core,
    /// `self` - same directory prefix in imports
    Self_,
    /// `super` - parent directory prefix in imports
    Super,

    // ─────────────────────────────────────────────────────────────────────────
    // Literals
    // ─────────────────────────────────────────────────────────────────────────
    /// Identifier
    Ident,
    /// Number literal
    Number,
    /// Canonical uppercase UUID literal (`8-4-4-4-12` hex), used in
    /// `unique(...)` nominal struct and enum declarations.
    Uuid,
    /// String literal (complete, no interpolation)
    String,
    /// String start (before first interpolation)
    StringStart,
    /// String middle (between interpolations)
    StringMiddle,
    /// String end (after last interpolation)
    StringEnd,

    // ─────────────────────────────────────────────────────────────────────────
    // Operators
    // ─────────────────────────────────────────────────────────────────────────
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `==`
    EqEq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `!`
    Bang,
    /// `=`
    Eq,
    /// `=>`
    FatArrow,
    /// `->`
    Arrow,
    /// `_`
    Underscore,

    // ─────────────────────────────────────────────────────────────────────────
    // Punctuation
    // ─────────────────────────────────────────────────────────────────────────
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `,`
    Comma,
    /// `;`
    Semi,
    /// `:`
    Colon,
    /// `::`
    ColonColon,
    /// `.`
    Dot,

    // ─────────────────────────────────────────────────────────────────────────
    // Trivia (preserved for CST)
    // ─────────────────────────────────────────────────────────────────────────
    /// Whitespace (spaces, tabs, newlines)
    Whitespace,
    /// Line comment (`// ...`)
    Comment,
    /// Doc comment (`/// ...`)
    DocComment,
    /// Inner doc comment (`//! ...`)
    InnerDocComment,

    // ─────────────────────────────────────────────────────────────────────────
    // Special
    // ─────────────────────────────────────────────────────────────────────────
    /// End of file
    Eof,
    /// Error token (for error recovery)
    Error,
}

impl TokenKind {
    /// Check if this token is trivia (whitespace or comment).
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace | Self::Comment | Self::DocComment | Self::InnerDocComment
        )
    }

    /// Check if this token is a keyword.
    #[must_use]
    pub const fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::Fn
                | Self::Pub
                | Self::Let
                | Self::Const
                | Self::If
                | Self::Else
                | Self::Match
                | Self::True
                | Self::False
                | Self::Enum
                | Self::Type
                | Self::Ability
                | Self::Use
                | Self::With
                | Self::Handle
                | Self::Resume
                | Self::Sandbox
                | Self::Unique
                | Self::Trait
                | Self::Impl
                | Self::For
                | Self::Where
                | Self::Pkg
                | Self::Core
                | Self::Self_
                | Self::Super
        )
    }

    /// Get the keyword for an identifier, if any.
    #[must_use]
    pub fn keyword_from_str(s: &str) -> Option<Self> {
        match s {
            "fn" => Some(Self::Fn),
            "pub" => Some(Self::Pub),
            "let" => Some(Self::Let),
            "const" => Some(Self::Const),
            "if" => Some(Self::If),
            "else" => Some(Self::Else),
            "match" => Some(Self::Match),
            "true" => Some(Self::True),
            "false" => Some(Self::False),
            "enum" => Some(Self::Enum),
            "type" => Some(Self::Type),
            "struct" => Some(Self::Struct),
            "ability" => Some(Self::Ability),
            "use" => Some(Self::Use),
            "with" => Some(Self::With),
            "handle" => Some(Self::Handle),
            "resume" => Some(Self::Resume),
            "sandbox" => Some(Self::Sandbox),
            "unique" => Some(Self::Unique),
            "extern" => Some(Self::Extern),
            "trait" => Some(Self::Trait),
            "impl" => Some(Self::Impl),
            "for" => Some(Self::For),
            "where" => Some(Self::Where),
            "pkg" => Some(Self::Pkg),
            "core" => Some(Self::Core),
            "self" => Some(Self::Self_),
            "super" => Some(Self::Super),
            _ => None,
        }
    }

    /// Get the string representation of a keyword token.
    #[must_use]
    pub const fn as_keyword_str(self) -> Option<&'static str> {
        match self {
            Self::Fn => Some("fn"),
            Self::Pub => Some("pub"),
            Self::Let => Some("let"),
            Self::Const => Some("const"),
            Self::If => Some("if"),
            Self::Else => Some("else"),
            Self::Match => Some("match"),
            Self::True => Some("true"),
            Self::False => Some("false"),
            Self::Enum => Some("enum"),
            Self::Type => Some("type"),
            Self::Struct => Some("struct"),
            Self::Ability => Some("ability"),
            Self::Use => Some("use"),
            Self::With => Some("with"),
            Self::Handle => Some("handle"),
            Self::Resume => Some("resume"),
            Self::Sandbox => Some("sandbox"),
            Self::Unique => Some("unique"),
            Self::Extern => Some("extern"),
            Self::Trait => Some("trait"),
            Self::Impl => Some("impl"),
            Self::For => Some("for"),
            Self::Where => Some("where"),
            Self::Pkg => Some("pkg"),
            Self::Core => Some("core"),
            Self::Self_ => Some("self"),
            Self::Super => Some("super"),
            _ => None,
        }
    }

    /// Get all keyword strings.
    #[must_use]
    pub const fn all_keywords() -> &'static [&'static str] {
        &[
            "fn", "pub", "let", "const", "if", "else", "match", "true", "false", "enum", "type",
            "struct", "ability", "use", "with", "handle", "resume", "sandbox", "unique", "extern",
            "trait", "impl", "for", "where", "pkg", "core", "self", "super",
        ]
    }

    /// Get all built-in type names.
    #[must_use]
    pub const fn builtin_types() -> &'static [&'static str] {
        &[
            "Number", "String", "Bool", "Binary", "List", "Map", "Set", "Option", "Result",
        ]
    }

    /// Get all built-in ability names.
    #[must_use]
    pub const fn builtin_abilities() -> &'static [&'static str] {
        &[
            "Stdio",
            "Exception",
            "Time",
            "Random",
            "Log",
            "FileSystem",
            "Tcp",
        ]
    }
}

/// The lexer converts source text into tokens.
pub struct Lexer<'src> {
    source: &'src str,
    chars: Peekable<CharIndices<'src>>,
    pos: usize,
    /// Stack of interpolation brace depths for nested string interpolations.
    interpolation_depth: Vec<u32>,
}

impl<'src> Lexer<'src> {
    /// Create a new lexer for the given source.
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.char_indices().peekable(),
            pos: 0,
            interpolation_depth: Vec::new(),
        }
    }

    /// Get the current position in the source.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Peek at the current character without consuming it.
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|(_, c)| *c)
    }

    /// Peek at the next character (after current).
    fn peek_next(&self) -> Option<char> {
        let mut chars = self.chars.clone();
        chars.next();
        chars.peek().map(|(_, c)| *c)
    }

    /// Consume and return the current character.
    fn advance(&mut self) -> Option<char> {
        if let Some((pos, c)) = self.chars.next() {
            self.pos = pos + c.len_utf8();
            Some(c)
        } else {
            None
        }
    }

    /// Consume if the current character matches.
    fn consume_if(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Get a span from start to current position.
    fn span_from(&self, start: usize) -> Span {
        Span::new(
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(self.pos).unwrap_or(u32::MAX),
        )
    }

    /// Get the text from start to current position.
    fn text_from(&self, start: usize) -> &'src str {
        &self.source[start..self.pos]
    }

    /// Make a token from start position to current.
    fn make_token(&self, kind: TokenKind, start: usize) -> Token {
        Token::new(kind, self.span_from(start), self.text_from(start))
    }

    /// Tokenize the entire source into a vector of tokens.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if an invalid token is encountered.
    pub fn tokenize(&mut self) -> Result<Vec<Token>, ParseError> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token()?;
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    /// Get the next token.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if an invalid token is encountered.
    #[allow(clippy::too_many_lines)]
    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        // Handle string interpolation continuation
        if !self.interpolation_depth.is_empty()
            && let Some('}') = self.peek()
        {
            // Check if this closes an interpolation
            if let Some(&depth) = self.interpolation_depth.last() {
                if depth == 0 {
                    // Depth 0 means the `}` closes the interpolation itself;
                    // consume it and resume lexing the surrounding string.
                    self.advance(); // consume '}'
                    return self.lex_string_continuation();
                }
                // Otherwise the `}` closes a brace-delimited construct nested
                // inside the interpolation (block, record literal, ...). Emit a
                // real RBrace: consume the `}` so it isn't re-read as the
                // interpolation terminator, and span it to the `}` itself.
                let start = self.pos;
                self.advance(); // consume '}'
                let token = self.make_token(TokenKind::RBrace, start);
                if let Some(top) = self.interpolation_depth.last_mut() {
                    *top -= 1;
                }
                return Ok(token);
            }
        }

        let Some(c) = self.peek() else {
            return Ok(Token::new(TokenKind::Eof, self.span_from(self.pos), ""));
        };

        let start = self.pos;

        match c {
            // Whitespace
            ' ' | '\t' | '\n' | '\r' => self.lex_whitespace(start),

            // Comments or division
            '/' => {
                self.advance();
                if self.consume_if('/') {
                    self.lex_line_comment(start)
                } else {
                    Ok(self.make_token(TokenKind::Slash, start))
                }
            }

            // UUID literals. Checked before identifiers and numbers because a
            // canonical UUID can begin with either a hex letter (`A`-`F`) or a
            // digit, and the whole `8-4-4-4-12` shape must be consumed as one
            // token rather than split into idents, numbers, and minuses.
            _ if self.at_uuid() => self.lex_uuid(start),

            // Identifiers and keywords
            'a'..='z' | 'A'..='Z' | '_' => self.lex_identifier(start),

            // Numbers
            '0'..='9' => self.lex_number(start),

            // Strings
            '"' => self.lex_string(start),

            // Operators and punctuation
            '+' => {
                self.advance();
                Ok(self.make_token(TokenKind::Plus, start))
            }
            '-' => {
                self.advance();
                if self.consume_if('>') {
                    Ok(self.make_token(TokenKind::Arrow, start))
                } else {
                    Ok(self.make_token(TokenKind::Minus, start))
                }
            }
            '*' => {
                self.advance();
                Ok(self.make_token(TokenKind::Star, start))
            }
            '%' => {
                self.advance();
                Ok(self.make_token(TokenKind::Percent, start))
            }
            '=' => {
                self.advance();
                if self.consume_if('=') {
                    Ok(self.make_token(TokenKind::EqEq, start))
                } else if self.consume_if('>') {
                    Ok(self.make_token(TokenKind::FatArrow, start))
                } else {
                    Ok(self.make_token(TokenKind::Eq, start))
                }
            }
            '!' => {
                self.advance();
                if self.consume_if('=') {
                    Ok(self.make_token(TokenKind::Ne, start))
                } else {
                    Ok(self.make_token(TokenKind::Bang, start))
                }
            }
            '<' => {
                self.advance();
                if self.consume_if('=') {
                    Ok(self.make_token(TokenKind::Le, start))
                } else {
                    Ok(self.make_token(TokenKind::Lt, start))
                }
            }
            '>' => {
                self.advance();
                if self.consume_if('=') {
                    Ok(self.make_token(TokenKind::Ge, start))
                } else {
                    Ok(self.make_token(TokenKind::Gt, start))
                }
            }
            '&' => {
                self.advance();
                if self.consume_if('&') {
                    Ok(self.make_token(TokenKind::AndAnd, start))
                } else {
                    Err(ParseError::new(
                        ParseErrorKind::UnexpectedChar('&'),
                        self.span_from(start),
                    ))
                }
            }
            '|' => {
                self.advance();
                if self.consume_if('|') {
                    Ok(self.make_token(TokenKind::OrOr, start))
                } else {
                    Err(ParseError::new(
                        ParseErrorKind::UnexpectedChar('|'),
                        self.span_from(start),
                    ))
                }
            }

            // Punctuation
            '(' => {
                self.advance();
                Ok(self.make_token(TokenKind::LParen, start))
            }
            ')' => {
                self.advance();
                Ok(self.make_token(TokenKind::RParen, start))
            }
            '{' => {
                self.advance();
                // Track brace depth for interpolation
                if let Some(depth) = self.interpolation_depth.last_mut() {
                    *depth += 1;
                }
                Ok(self.make_token(TokenKind::LBrace, start))
            }
            '}' => {
                self.advance();
                Ok(self.make_token(TokenKind::RBrace, start))
            }
            '[' => {
                self.advance();
                Ok(self.make_token(TokenKind::LBracket, start))
            }
            ']' => {
                self.advance();
                Ok(self.make_token(TokenKind::RBracket, start))
            }
            ',' => {
                self.advance();
                Ok(self.make_token(TokenKind::Comma, start))
            }
            ';' => {
                self.advance();
                Ok(self.make_token(TokenKind::Semi, start))
            }
            ':' => {
                self.advance();
                if self.consume_if(':') {
                    Ok(self.make_token(TokenKind::ColonColon, start))
                } else {
                    Ok(self.make_token(TokenKind::Colon, start))
                }
            }
            '.' => {
                self.advance();
                Ok(self.make_token(TokenKind::Dot, start))
            }

            _ => {
                self.advance();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedChar(c),
                    self.span_from(start),
                ))
            }
        }
    }

    /// Lex whitespace characters. Returns Result for API consistency with other
    /// lex methods, even though whitespace lexing never fails.
    #[allow(clippy::unnecessary_wraps)]
    fn lex_whitespace(&mut self, start: usize) -> Result<Token, ParseError> {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\n' | '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }
        Ok(self.make_token(TokenKind::Whitespace, start))
    }

    /// Lex a line comment (regular, doc, or inner doc). Returns Result for API
    /// consistency with other lex methods.
    #[allow(clippy::unnecessary_wraps)]
    fn lex_line_comment(&mut self, start: usize) -> Result<Token, ParseError> {
        // Already consumed "//"
        // Check for doc comment markers
        let kind = match self.peek() {
            Some('/') => {
                // `///` - outer doc comment
                self.advance();
                TokenKind::DocComment
            }
            Some('!') => {
                // `//!` - inner doc comment
                self.advance();
                TokenKind::InnerDocComment
            }
            _ => TokenKind::Comment,
        };

        // Consume rest of line
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.advance();
        }
        Ok(self.make_token(kind, start))
    }

    /// Lex an identifier or keyword. Returns Result for API consistency with
    /// other lex methods.
    #[allow(clippy::unnecessary_wraps)]
    fn lex_identifier(&mut self, start: usize) -> Result<Token, ParseError> {
        while let Some(c) = self.peek() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => {
                    self.advance();
                }
                _ => break,
            }
        }

        let text = self.text_from(start);

        // Check for underscore as wildcard pattern
        if text == "_" {
            return Ok(self.make_token(TokenKind::Underscore, start));
        }

        // Check for keywords
        let kind = TokenKind::keyword_from_str(text).unwrap_or(TokenKind::Ident);
        Ok(self.make_token(kind, start))
    }

    /// Look ahead from the current position to decide whether the upcoming
    /// characters form a canonical uppercase UUID literal without consuming
    /// anything.
    ///
    /// A UUID is `HEX{8}-HEX{4}-HEX{4}-HEX{4}-HEX{12}` where `HEX` is a digit
    /// or an *uppercase* `A`-`F`. Lowercase hex is deliberately excluded so
    /// that ordinary identifiers and numbers (and any future lowercase `0x`
    /// hex literals) remain unambiguous — only fully-uppercase UUIDs are lexed
    /// as a single token. The match must not be immediately followed by an
    /// identifier/hex continuation character, otherwise it is part of a longer
    /// token and not a UUID.
    fn at_uuid(&self) -> bool {
        const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
        let mut chars = self.chars.clone();
        for (group, &len) in GROUPS.iter().enumerate() {
            if group > 0 && !matches!(chars.next(), Some((_, '-'))) {
                return false;
            }
            for _ in 0..len {
                if !matches!(chars.next(), Some((_, '0'..='9' | 'A'..='F'))) {
                    return false;
                }
            }
        }
        // Reject if a further identifier/hex/UUID character follows, e.g. a
        // sixth group or a trailing letter — that is a longer, malformed token.
        !matches!(
            chars.peek(),
            Some((_, c)) if c.is_ascii_alphanumeric() || *c == '_' || *c == '-'
        )
    }

    /// Consume a canonical uppercase UUID literal. Only called once `at_uuid`
    /// has confirmed the full `8-4-4-4-12` shape, so the 32 hex digits and 4
    /// dashes (36 characters) are guaranteed present.
    ///
    /// Returns `Result` for API consistency with the other lex methods, even
    /// though UUID lexing never fails once `at_uuid` has matched.
    #[allow(clippy::unnecessary_wraps)]
    fn lex_uuid(&mut self, start: usize) -> Result<Token, ParseError> {
        for _ in 0..36 {
            self.advance();
        }
        Ok(self.make_token(TokenKind::Uuid, start))
    }

    /// Look ahead from the current `e`/`E` to decide whether it introduces a
    /// well-formed exponent (optional sign, then at least one digit) without
    /// consuming anything.
    fn has_exponent_digits(&self) -> bool {
        let mut chars = self.chars.clone();
        chars.next(); // skip the 'e'/'E'
        if let Some((_, '+' | '-')) = chars.peek() {
            chars.next();
        }
        matches!(chars.peek(), Some((_, '0'..='9')))
    }

    /// Lex a number literal. Returns Result for API consistency with other lex
    /// methods, even though number lexing never fails: a bare `e`/`E` with no
    /// following digit is simply not treated as an exponent.
    #[allow(clippy::unnecessary_wraps)]
    fn lex_number(&mut self, start: usize) -> Result<Token, ParseError> {
        // Integer part
        while let Some('0'..='9') = self.peek() {
            self.advance();
        }

        // Decimal part
        if self.peek() == Some('.') && matches!(self.peek_next(), Some('0'..='9')) {
            self.advance(); // consume '.'
            while let Some('0'..='9') = self.peek() {
                self.advance();
            }
        }

        // Exponent part. Only commit to it if it's a well-formed exponent —
        // `e`/`E`, an optional sign, then at least one digit. If no digit
        // follows, the `e` is not scientific notation: the number ends here and
        // the `e...` starts an identifier. This matters for hex sequences like
        // the UUID groups in `unique(2eb9553c-...)`, where treating `2e` as a
        // malformed float literal would abort tokenization on a valid UUID.
        if matches!(self.peek(), Some('e' | 'E')) && self.has_exponent_digits() {
            self.advance(); // consume 'e'/'E'
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }
            while let Some('0'..='9') = self.peek() {
                self.advance();
            }
        }

        Ok(self.make_token(TokenKind::Number, start))
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, ParseError> {
        self.advance(); // consume opening '"'

        let content_start = self.pos;

        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::new(
                        ParseErrorKind::UnterminatedString,
                        self.span_from(start),
                    ));
                }
                Some('"') => {
                    self.advance();
                    return Ok(self.make_token(TokenKind::String, start));
                }
                Some('$') if self.peek_next() == Some('{') => {
                    // Start interpolation
                    let kind = if content_start == start + 1 {
                        TokenKind::StringStart
                    } else {
                        TokenKind::StringMiddle
                    };
                    let token = self.make_token(kind, start);
                    self.advance(); // consume '$'
                    self.advance(); // consume '{'
                    self.interpolation_depth.push(0);
                    return Ok(token);
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('n' | 'r' | 't' | '\\' | '"' | '$') => {
                            self.advance();
                        }
                        Some(c) => {
                            return Err(ParseError::new(
                                ParseErrorKind::InvalidEscape(c),
                                self.span_from(self.pos - 1),
                            ));
                        }
                        None => {
                            return Err(ParseError::new(
                                ParseErrorKind::UnterminatedString,
                                self.span_from(start),
                            ));
                        }
                    }
                }
                Some(_) => {
                    self.advance();
                }
            }
        }
    }

    fn lex_string_continuation(&mut self) -> Result<Token, ParseError> {
        // We just consumed '}' that closes an interpolation
        self.interpolation_depth.pop();

        let start = self.pos - 1; // include the '}'

        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::new(
                        ParseErrorKind::UnterminatedString,
                        self.span_from(start),
                    ));
                }
                Some('"') => {
                    self.advance();
                    return Ok(self.make_token(TokenKind::StringEnd, start));
                }
                Some('$') if self.peek_next() == Some('{') => {
                    // Another interpolation
                    let token = self.make_token(TokenKind::StringMiddle, start);
                    self.advance(); // consume '$'
                    self.advance(); // consume '{'
                    self.interpolation_depth.push(0);
                    return Ok(token);
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('n' | 'r' | 't' | '\\' | '"' | '$') => {
                            self.advance();
                        }
                        Some(c) => {
                            return Err(ParseError::new(
                                ParseErrorKind::InvalidEscape(c),
                                self.span_from(self.pos - 1),
                            ));
                        }
                        None => {
                            return Err(ParseError::new(
                                ParseErrorKind::UnterminatedString,
                                self.span_from(start),
                            ));
                        }
                    }
                }
                Some(_) => {
                    self.advance();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "lexer_tests.rs"]
mod tests;
