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
    /// `unique(...)` nominal type declarations.
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
            "ability" => Some(Self::Ability),
            "use" => Some(Self::Use),
            "with" => Some(Self::With),
            "handle" => Some(Self::Handle),
            "resume" => Some(Self::Resume),
            "sandbox" => Some(Self::Sandbox),
            "unique" => Some(Self::Unique),
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
            Self::Ability => Some("ability"),
            Self::Use => Some("use"),
            Self::With => Some("with"),
            Self::Handle => Some("handle"),
            Self::Resume => Some("resume"),
            Self::Sandbox => Some("sandbox"),
            Self::Unique => Some("unique"),
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
            "ability", "use", "with", "handle", "resume", "sandbox", "unique", "trait", "impl",
            "for", "where", "pkg", "core", "self", "super",
        ]
    }

    /// Get all built-in type names.
    #[must_use]
    pub const fn builtin_types() -> &'static [&'static str] {
        &[
            "number", "string", "bool", "Bytes", "List", "Map", "Set", "Option", "Result",
        ]
    }

    /// Get all built-in ability names.
    #[must_use]
    pub const fn builtin_abilities() -> &'static [&'static str] {
        &[
            "Console",
            "Exception",
            "Time",
            "Random",
            "Log",
            "FileSystem",
            "Network",
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
            if let Some(depth) = self.interpolation_depth.last_mut() {
                if *depth == 0 {
                    // This closes the interpolation, continue string
                    self.advance(); // consume '}'
                    return self.lex_string_continuation();
                }
                *depth -= 1;
                let start = self.pos - 1;
                return Ok(self.make_token(TokenKind::RBrace, start));
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
mod tests {
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
        assert_eq!(lex("ability"), vec![TokenKind::Ability, TokenKind::Eof]);
        assert_eq!(lex("use"), vec![TokenKind::Use, TokenKind::Eof]);
        assert_eq!(lex("with"), vec![TokenKind::With, TokenKind::Eof]);
        assert_eq!(lex("handle"), vec![TokenKind::Handle, TokenKind::Eof]);
        assert_eq!(lex("resume"), vec![TokenKind::Resume, TokenKind::Eof]);
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
}
