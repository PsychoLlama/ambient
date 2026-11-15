//! What a world takes time over: work it is asked to begin, and gives
//! the outcome of later. See "Native side" in `spec/externs.md`.
//!
//! A handler that waits does not block: it begins the work under a
//! ticket, its task suspends, and the program takes each outcome from
//! the world when no task can run without one.

use std::cell::RefCell;
use std::rc::Rc;

use super::{Failure, Ran, World, error, optional, output};
use crate::backend::native::{Answer, Waits};
use crate::value::Value;

/// Something a world is asked to begin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// Let this many milliseconds pass.
    Sleep(i64),
    /// Run a command with arguments, until it is over.
    Run { command: String, args: Vec<String> },
    /// Read the next line of standard input.
    ReadLine,
}

/// What came of work a world began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    Slept,
    /// What the command gave, or why it could not be started.
    Ran(Result<Ran, Failure>),
    /// The line, without its ending, or nothing at the end of input.
    Line(Option<String>),
}

impl Done {
    /// What the handler that began the work gives its program.
    fn value(self) -> Value {
        match self {
            Self::Slept => Value::Null,
            Self::Ran(ran) => ran.map_or_else(|failure| error("process", &failure), output),
            Self::Line(line) => optional(line),
        }
    }
}

/// The answers of one world, as a program takes them.
pub(super) struct Outcomes(pub(super) Rc<RefCell<dyn World>>);

impl Waits for Outcomes {
    fn next(&mut self) -> Option<Answer> {
        let (ticket, done) = self.0.borrow_mut().next()?;
        Some((ticket, Ok(done.value())))
    }

    fn forsake(&mut self, ticket: u64) {
        self.0.borrow_mut().forsake(ticket);
    }
}
