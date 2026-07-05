//! Parser for the Ambient programming language.
//!
//! This crate implements Milestone 10 of the Ambient language specification:
//! - Lexer (tokenization)
//! - CST (Concrete Syntax Tree) with preserved whitespace and comments
//! - Recursive descent parser with error recovery
//! - Source spans on all nodes
//! - String interpolation parsing
//!
//! The parser produces a CST which can then be lowered to the AST defined
//! in `ambient-engine` for type checking and compilation.
//!
//! # Architecture
//!
//! ```text
//! Source (.ab)
//!      │
//!      ▼
//! ┌─────────┐
//! │  Lexer  │ ─── Tokenizes source into Token stream
//! └────┬────┘
//!      │
//!      ▼
//! ┌─────────┐
//! │ Parser  │ ─── Builds CST from tokens (recursive descent)
//! └────┬────┘
//!      │
//!      ▼
//! ┌─────────┐
//! │  Lower  │ ─── Converts CST to AST (ambient-engine::ast)
//! └────┬────┘
//!      │
//!      ▼
//!    AST (ready for type inference)
//! ```
//!
//! # Example
//!
//! ```
//! use ambient_parser::{parse, parse_expr};
//!
//! // Parse a complete module
//! let source = "fn add(x: Number, y: Number): Number { x + y }";
//! let module = parse(source).expect("parse error");
//!
//! // Parse a single expression
//! let expr = parse_expr("1 + 2 * 3").expect("parse error");
//! ```

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used,
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used))]

mod cst;
mod error;
mod lexer;
mod lower;
mod parser;

pub use cst::{
    CstAbilityDef, CstAbilityMethod, CstConstDef, CstEnumDef, CstEnumVariant, CstExpr, CstExprKind,
    CstFunctionDef, CstHandleExpr, CstHandler, CstItem, CstItemKind, CstLambda, CstLetBinding,
    CstMatchArm, CstModule, CstParam, CstPattern, CstPatternKind, CstReplInput, CstStmt,
    CstStmtKind, CstTypeAliasDef, CstTypeExpr, CstTypeExprKind, CstUseDef, CstUseTree,
    CstUseTreeKind, Trivia, TriviaKind,
};
pub use error::{ParseError, ParseErrorKind};
pub use lexer::{Lexer, Token, TokenKind};
pub use lower::{lower_module, lower_module_recovering};
pub use parser::Parser;

use ambient_engine::ast::{Expr, Item, Module};

/// The outcome of parsing with error recovery: the partial module plus every
/// error encountered.
///
/// The module contains every item that parsed and lowered cleanly. When
/// `errors` is empty the module is identical to what [`parse`] returns.
#[derive(Debug, Clone)]
pub struct RecoveredParse {
    /// The partial (or complete) module.
    pub module: Module,
    /// All parse and lowering errors, in source order.
    pub errors: Vec<ParseError>,
}

/// REPL input after lowering from CST to AST.
#[derive(Debug, Clone)]
pub enum ReplInput {
    /// Item definitions (function, const, type, etc.). A single source
    /// item usually lowers to one AST item; a `use` tree flattens to one
    /// per imported leaf.
    Items(Vec<Item>),
    /// An expression to evaluate. Boxed to keep the variants close in
    /// size.
    Expr(Box<Expr>),
}

/// Parse source code into an AST module.
///
/// This is the main entry point for parsing Ambient source files.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains syntax errors.
pub fn parse(source: &str) -> Result<Module, ParseError> {
    let cst = parse_to_cst(source)?;
    lower_module(&cst)
}

/// Parse source code with error recovery, returning a partial module plus
/// every error encountered.
///
/// Unlike [`parse`], this never fails outright on a syntax error: items that
/// fail to parse or lower are skipped, and analysis can proceed on the rest.
/// This is the entry point for IDE tooling, which routinely sees files
/// mid-edit. A lexer error (unterminated string, invalid escape) still
/// abandons the whole file — the token stream is unusable — yielding an
/// empty module plus that single error.
#[must_use]
pub fn parse_recovering(source: &str) -> RecoveredParse {
    let mut parser = match Parser::new(source) {
        Ok(parser) => parser,
        Err(e) => {
            return RecoveredParse {
                module: Module {
                    name: "".into(),
                    doc: None,
                    items: Vec::new(),
                },
                errors: vec![e],
            };
        }
    };
    let (cst, mut errors) = parser.parse_module_recovering();
    let (module, lowering_errors) = lower_module_recovering(&cst);
    errors.extend(lowering_errors);
    errors.sort_by_key(|e| (e.span.start, e.span.end));
    RecoveredParse { module, errors }
}

/// Parse source code into a CST module.
///
/// The CST preserves whitespace and comments for tooling support.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains lexer or syntax errors.
pub fn parse_to_cst(source: &str) -> Result<CstModule, ParseError> {
    let mut parser = Parser::new(source)?;
    parser.parse_module()
}

/// Parse a single expression from source.
///
/// # Errors
///
/// Returns a `ParseError` if the source is not a valid expression.
pub fn parse_expr(source: &str) -> Result<Expr, ParseError> {
    let cst = parse_expr_to_cst(source)?;
    lower::lower_expr(&cst)
}

/// Parse a single expression to CST.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains lexer or syntax errors.
pub fn parse_expr_to_cst(source: &str) -> Result<CstExpr, ParseError> {
    let mut parser = Parser::new(source)?;
    parser.parse_expression()
}

/// Parse a type expression from source.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains lexer or syntax errors.
pub fn parse_type(source: &str) -> Result<CstTypeExpr, ParseError> {
    let mut parser = Parser::new(source)?;
    parser.parse_type()
}

/// Parse REPL input (either an item definition or an expression).
///
/// This is used by the REPL to support interactive definition of functions,
/// constants, types, and abilities as well as expression evaluation.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains lexer or syntax errors.
pub fn parse_repl_input(source: &str) -> Result<ReplInput, ParseError> {
    let mut parser = Parser::new(source)?;
    let cst = parser.parse_repl_input()?;
    match cst {
        CstReplInput::Item(item) => Ok(ReplInput::Items(lower::lower_item(&item)?)),
        CstReplInput::Expr(expr) => Ok(ReplInput::Expr(Box::new(lower::lower_expr(&expr)?))),
    }
}
