//! What a world takes time over: work it is asked to begin, and gives
//! the outcome of later. See "Native side" in `docs/spec/externs.md`.
//!
//! A handler that waits does not block: it begins the work under a
//! ticket, its task suspends, and the program takes each outcome from
//! the world when no task can run without one.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::Command;
use super::files;
use super::http::{self, Incoming, Request, Response, Streamed};
use super::net::{self, Held, Reach, Reached};
use super::signals::Signal;
use super::trust::Trust;
use super::watching::Change;
use super::{Failure, PACKAGE, Ran, World, error, optional, output};
use crate::native::{Answer, Posts, Waits};
use crate::value::Value;

/// Something a world is asked to begin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// Let this many milliseconds pass.
    Sleep(i64),
    /// Run a command, until it is over.
    Run(Command),
    /// Read the next line of standard input.
    ReadLine,
    /// Read what standard input has next, as bytes: at most this many.
    Read(i64),
    /// Send a request, and receive the response to it.
    Send(Request),
    /// Send a request, and receive the head of the response to it,
    /// with its body still to arrive.
    Open(Request, Option<Trust>),
    /// Take the next request a server was sent.
    Accept(i64),
    /// Something of a stream or a socket.
    Reach(Reach),
    /// Wait until the program is asked something from outside.
    Signal,
    /// Take the next change a watcher saw.
    Change(i64),
}

/// A line of input without its ending, or nothing at the end of input.
/// One that is not text is a failure: the program reads text or stops.
pub type Line = Result<Option<String>, Failure>;

/// Bytes of input, or nothing at the end of input.
pub type Chunk = Result<Option<Vec<u8>>, Failure>;

/// What came of work a world began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    Slept,
    /// What the command gave, or why it could not be started.
    Ran(Result<Ran, Failure>),
    /// The line, or why there is none to give.
    Line(Line),
    /// The bytes, or why there are none to give.
    Read(Chunk),
    /// The response, or why there was none.
    Sent(Result<Response, Failure>),
    /// The head of the response and its body, a stream, or why there
    /// was none.
    Opened(Result<Streamed, Failure>),
    /// The request, or nothing once the server is closed.
    Accepted(Option<Incoming>),
    /// What came of something of a stream or a socket.
    Reached(Reached),
    /// What the program was asked from outside.
    Signalled(Signal),
    /// The change, or nothing once the watcher is closed.
    Changed(Option<Change>),
}

impl Done {
    /// What the handler that began the work gives its program, or
    /// what went wrong in it, which ends the program.
    fn answer(self) -> Result<Value, String> {
        Ok(match self {
            Self::Slept => Value::Null,
            Self::Ran(ran) => ran.map_or_else(|failure| error("process", &failure), output),
            Self::Line(line) => optional(line.map_err(|failure| failure.message)?),
            Self::Read(chunk) => chunk
                .map_err(|failure| failure.message)?
                .map_or(Value::Null, Value::Bytes),
            Self::Sent(sent) => http::sent(sent),
            Self::Opened(opened) => {
                opened.map_or_else(|failure| error("http", &failure), Streamed::value)
            }
            Self::Accepted(incoming) => incoming.map_or(Value::Null, Incoming::value),
            Self::Reached(reached) => reached.value(),
            Self::Changed(change) => change.map_or(Value::Null, files::change),
            Self::Signalled(signal) => Value::Tag {
                package: PACKAGE.to_owned(),
                tag: format!("process.{}", signal.name()),
            },
        })
    }
}

/// The answers of one world, as a program takes them.
pub(super) struct Outcomes(pub(super) Rc<RefCell<dyn World>>);

impl Waits for Outcomes {
    fn next(&mut self) -> Option<Answer> {
        let (ticket, done) = self.0.borrow_mut().next()?;
        Some((ticket, done.answer()))
    }

    fn forsake(&mut self, ticket: u64) {
        self.0.borrow_mut().forsake(ticket);
    }

    fn attend(&mut self, posts: Arc<Posts>) {
        self.0.borrow_mut().attend(posts);
    }

    /// What a handle stood for is given up with it: a server stops
    /// listening, and a request is let go.
    fn dropped(&mut self, package: &str, brand: &str, number: u64) {
        let number = number.cast_signed();
        match (package, brand) {
            (PACKAGE, http::LISTENER) => self.0.borrow_mut().close(number),
            (PACKAGE, http::REPLY) => self.0.borrow_mut().abandon(number),
            (PACKAGE, files::WATCHER) => self.0.borrow_mut().shut(Held::Watcher(number)),
            (PACKAGE, brand) => {
                if let Some(held) = net::dropped(brand, number) {
                    self.0.borrow_mut().shut(held);
                }
            }
            _ => {}
        }
    }
}
