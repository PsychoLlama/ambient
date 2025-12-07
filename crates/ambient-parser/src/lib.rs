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
//! let source = "fn add(x: number, y: number): number { x + y }";
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
mod resolve;

pub use cst::{
    CstAbilityDef, CstAbilityMethod, CstConstDef, CstEnumDef, CstEnumVariant, CstExpr, CstExprKind,
    CstFunctionDef, CstHandleExpr, CstHandler, CstItem, CstItemKind, CstLambda, CstLetBinding,
    CstMatchArm, CstModule, CstParam, CstPattern, CstPatternKind, CstReplInput, CstStmt,
    CstStmtKind, CstTypeAliasDef, CstTypeExpr, CstTypeExprKind, CstUseDef, CstUseKind,
    CstUsePrefix, Trivia, TriviaKind,
};
pub use error::{ParseError, ParseErrorKind};
pub use lexer::{Lexer, Token, TokenKind};
pub use lower::lower_module;
pub use parser::Parser;
pub use resolve::{DefId, Resolver};

use ambient_engine::ast::{Expr, Item, Module};

/// REPL input after lowering from CST to AST.
#[derive(Debug, Clone)]
pub enum ReplInput {
    /// An item definition (function, const, type, etc.).
    Item(Item),
    /// An expression to evaluate.
    Expr(Expr),
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

/// Parse source code into a CST module.
///
/// The CST preserves whitespace and comments for tooling support.
///
/// # Errors
///
/// Returns a `ParseError` if the source contains syntax errors.
pub fn parse_to_cst(source: &str) -> Result<CstModule, ParseError> {
    let mut parser = Parser::new(source);
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
/// Returns a `ParseError` if the source is not a valid expression.
pub fn parse_expr_to_cst(source: &str) -> Result<CstExpr, ParseError> {
    let mut parser = Parser::new(source);
    parser.parse_expression()
}

/// Parse a type expression from source.
///
/// # Errors
///
/// Returns a `ParseError` if the source is not a valid type.
pub fn parse_type(source: &str) -> Result<CstTypeExpr, ParseError> {
    let mut parser = Parser::new(source);
    parser.parse_type()
}

/// Parse REPL input (either an item definition or an expression).
///
/// This is used by the REPL to support interactive definition of functions,
/// constants, types, and abilities as well as expression evaluation.
///
/// # Errors
///
/// Returns a `ParseError` if the source is not a valid item or expression.
pub fn parse_repl_input(source: &str) -> Result<ReplInput, ParseError> {
    let mut parser = Parser::new(source);
    let cst = parser.parse_repl_input()?;
    match cst {
        CstReplInput::Item(item) => Ok(ReplInput::Item(lower::lower_item(&item)?)),
        CstReplInput::Expr(expr) => Ok(ReplInput::Expr(lower::lower_expr(&expr)?)),
    }
}
