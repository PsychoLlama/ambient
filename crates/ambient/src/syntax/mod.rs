//! Source text, read into surface trees. See `spec/syntax.md`.
//!
//! The reader is one more client of the builders: what it makes is a
//! [`Module`] a builder accepted, and nothing downstream can tell it from
//! one built by hand. It resolves nothing. The one thing it knows about
//! names is which are locals of the function being read, which is what
//! tells `record.field` from `module.definition`.

mod lex;
mod parse;

use std::fmt;

use crate::surface::Module;

/// A place in a source file, counted from one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// Why text is not a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub at: Position,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Position { line, column } = self.at;
        write!(f, "{line}:{column}: {}", self.message)
    }
}

impl std::error::Error for SyntaxError {}

/// The module a source file is.
///
/// # Errors
///
/// The text is not a module: where it stops being one, and why.
pub fn parse(source: &str) -> Result<Module, SyntaxError> {
    let lexed = lex::lex(source)?;
    // A tree may nest as deeply as the text does, and reading recurses.
    crate::lower::deep(|| parse::Parser::new(lexed).module())
}
