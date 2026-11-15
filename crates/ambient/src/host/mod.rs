//! The native side of the standard library's command-line target: what
//! `system.cli` binds each ability to. See `spec/targets.md`.
//!
//! Everything a program can do to the world goes through a [`World`], so
//! the same handlers serve a real process ([`System`]) and a test that
//! scripts what the world holds.

mod http;
mod network;
mod pending;
mod system;
mod waits;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub use http::{Incoming, Listening, Request, Response};
pub use system::System;
pub use waits::{Done, Work};

use crate::backend::native::Natives;
use crate::system::PACKAGE;
use crate::value::Value;

/// Why the world refused something. The modules whose abilities can be
/// refused each have a tag for every kind, under the same names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NotFound,
    Denied,
    /// Nothing answered: a host that was not found or would not connect.
    Unreachable,
    /// What was given or received cannot be carried as it is.
    Invalid,
    Failed,
}

impl Kind {
    /// What the tag that names it is called in each of those modules.
    fn name(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Denied => "denied",
            Self::Unreachable => "unreachable",
            Self::Invalid => "invalid",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
}

/// What a command that was run gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// What it exited with, or -1 if it was ended from outside.
    pub code: i64,
    pub out: String,
    pub err: String,
}

/// What a program run from a command line can reach.
pub trait World {
    /// Writes to standard output.
    fn write(&mut self, text: &str);
    /// Writes to standard error.
    fn warn(&mut self, text: &str);
    /// What the program was started with.
    fn args(&self) -> Vec<String>;
    fn variable(&self, name: &str) -> Option<String>;
    /// The text of a file.
    ///
    /// # Errors
    ///
    /// The file cannot be read as text.
    fn read(&mut self, path: &str) -> Result<String, Failure>;
    /// Makes a file hold `text`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    fn store(&mut self, path: &str, text: &str) -> Result<(), Failure>;
    /// The names of what a directory holds, in order.
    ///
    /// # Errors
    ///
    /// The directory cannot be read.
    fn list(&mut self, path: &str) -> Result<Vec<String>, Failure>;
    fn exists(&mut self, path: &str) -> bool;
    /// Takes away a file, or a directory and all it holds.
    ///
    /// # Errors
    ///
    /// Nothing is there, or it cannot be taken away.
    fn remove(&mut self, path: &str) -> Result<(), Failure>;
    /// Makes a directory, and those it is inside that are missing.
    ///
    /// # Errors
    ///
    /// The directory cannot be made.
    fn make_directory(&mut self, path: &str) -> Result<(), Failure>;
    /// The directory a path that does not say where it starts is in.
    fn directory(&self) -> Option<String>;
    /// Bits that nothing could have guessed.
    ///
    /// # Errors
    ///
    /// The system has none to give.
    fn random(&mut self) -> Result<u64, Failure>;
    /// Milliseconds since the start of 1970.
    fn now(&mut self) -> i64;
    /// Nanoseconds since a fixed moment.
    fn elapsed(&mut self) -> i64;
    /// Starts listening on a port of an address of this machine; port
    /// zero is any that is free.
    ///
    /// # Errors
    ///
    /// The address cannot be listened on.
    fn listen(&mut self, host: &str, port: i64) -> Result<Listening, Failure>;
    /// Answers a request a server accepted.
    ///
    /// # Errors
    ///
    /// The request was answered already, or the answer cannot be sent.
    fn respond(&mut self, incoming: i64, response: Response) -> Result<(), Failure>;
    /// Stops a server listening: whatever waits to accept is given
    /// nothing.
    fn close(&mut self, server: i64);
    /// Begins work that takes time. What comes of it is given by
    /// [`World::next`], under `ticket`.
    fn begin(&mut self, ticket: u64, work: Work);
    /// What came of the work that is over soonest, waited for if none
    /// is yet. None if no work is under way.
    fn next(&mut self) -> Option<(u64, Done)>;
    /// Stops the work of `ticket`, if it can be stopped: nothing waits
    /// for it any more.
    fn forsake(&mut self, ticket: u64);
}

/// The error a failure is, as `ambient.Error` describes one: its kind
/// is the tag for it of `module`, whose ability was refused.
fn error(module: &str, failure: &Failure) -> Value {
    let kind = Value::Tag {
        package: PACKAGE.to_owned(),
        tag: format!("{module}.{}", failure.kind.name()),
    };
    let fields = [
        ("kind", kind),
        ("message", Value::String(failure.message.clone())),
        ("cause", Value::Null),
        ("trace", Value::List(Vec::new())),
    ];
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

fn text(args: &[Value], position: usize) -> Result<&str, String> {
    match args.get(position) {
        Some(Value::String(text)) => Ok(text),
        _ => Err(format!("argument {position} is not a string")),
    }
}

fn int(args: &[Value], position: usize) -> Result<i64, String> {
    match args.get(position) {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(format!("argument {position} is not an integer")),
    }
}

fn texts(args: &[Value], position: usize) -> Result<Vec<String>, String> {
    let Some(Value::List(items)) = args.get(position) else {
        return Err(format!("argument {position} is not a list"));
    };
    (0..items.len())
        .map(|item| text(items, item).map(str::to_owned))
        .collect()
}

/// What a command gave, as `system.process.Output` describes it.
fn output(ran: Ran) -> Value {
    let fields = [
        ("code", Value::Int(ran.code)),
        ("out", Value::String(ran.out)),
        ("err", Value::String(ran.err)),
    ];
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

fn optional(text: Option<String>) -> Value {
    text.map_or(Value::Null, Value::String)
}

fn strings(items: Vec<String>) -> Value {
    Value::List(items.into_iter().map(Value::String).collect())
}

/// The native code for every handler of the command-line target, over
/// one world.
#[must_use]
pub fn natives(world: &Rc<RefCell<dyn World>>) -> Natives {
    use http::{Handler, Starter};
    // What takes time is begun, and answered when it is over.
    let starters: [(&str, Starter); 3] = [
        ("console_read_line", |_| Ok(Work::ReadLine)),
        ("clock_sleep", |args| Ok(Work::Sleep(int(args, 0)?))),
        ("process_run", |args| {
            Ok(Work::Run {
                command: text(args, 0)?.to_owned(),
                args: texts(args, 1)?,
            })
        }),
    ];
    let handlers: [(&str, Handler); 15] = [
        ("console_write", |world, args| {
            world.write(text(args, 0)?);
            Ok(Value::Null)
        }),
        ("console_print", |world, args| {
            world.write(&format!("{}\n", text(args, 0)?));
            Ok(Value::Null)
        }),
        ("console_warn", |world, args| {
            world.warn(&format!("{}\n", text(args, 0)?));
            Ok(Value::Null)
        }),
        ("env_args", |world, _| Ok(strings(world.args()))),
        ("env_variable", |world, args| {
            Ok(optional(world.variable(text(args, 0)?)))
        }),
        ("fs_read", |world, args| {
            let read = world.read(text(args, 0)?);
            Ok(read.map_or_else(|failure| error("fs", &failure), Value::String))
        }),
        ("fs_write", |world, args| {
            let stored = world.store(text(args, 0)?, text(args, 1)?);
            Ok(stored.map_or_else(|failure| error("fs", &failure), |()| Value::Null))
        }),
        ("fs_list", |world, args| {
            let listed = world.list(text(args, 0)?);
            Ok(listed.map_or_else(|failure| error("fs", &failure), strings))
        }),
        ("fs_exists", |world, args| {
            Ok(Value::Bool(world.exists(text(args, 0)?)))
        }),
        ("fs_remove", |world, args| {
            let removed = world.remove(text(args, 0)?);
            Ok(removed.map_or_else(|failure| error("fs", &failure), |()| Value::Null))
        }),
        ("fs_make_directory", |world, args| {
            let made = world.make_directory(text(args, 0)?);
            Ok(made.map_or_else(|failure| error("fs", &failure), |()| Value::Null))
        }),
        ("random_bits", |world, _| {
            let bits = world.random().map_err(|failure| failure.message)?;
            Ok(Value::Int(bits.cast_signed()))
        }),
        ("clock_now", |world, _| Ok(Value::Int(world.now()))),
        ("clock_elapsed", |world, _| Ok(Value::Int(world.elapsed()))),
        // What a root handler of an ability that does not return gives
        // is what the program gives.
        ("process_exit", |_, args| Ok(Value::Int(int(args, 0)?))),
    ];
    let outcomes = Rc::new(RefCell::new(waits::Outcomes(Rc::clone(world))));
    let natives = Natives::new().answered_by(outcomes);
    let handlers = handlers.into_iter().chain(http::HANDLERS);
    let starters = starters.into_iter().chain(http::STARTERS);
    let natives = handlers.fold(natives, |natives, (name, handler)| {
        let world = Rc::clone(world);
        natives.with(PACKAGE, name, move |args| {
            handler(&mut *world.borrow_mut(), args)
        })
    });
    starters.fold(natives, |natives, (name, starter)| {
        let world = Rc::clone(world);
        natives.reaching(PACKAGE, name, move |running, args| {
            let work = starter(args)?;
            world.borrow_mut().begin(running.later()?, work);
            Ok(Value::Null)
        })
    })
}
