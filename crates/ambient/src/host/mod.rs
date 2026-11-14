//! The native side of the standard library's command-line target: what
//! `ambient.system.cli` binds each ability to. See `spec/targets.md`.
//!
//! Everything a program can do to the world goes through a [`World`], so
//! the same handlers serve a real process ([`System`]) and a test that
//! scripts what the world holds.

mod system;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

pub use system::System;

use crate::backend::native::Natives;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;

/// Why the world refused something done to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NotFound,
    Denied,
    Failed,
}

impl Kind {
    /// The tag of `ambient.fs` that names it.
    fn tag(self) -> &'static str {
        match self {
            Self::NotFound => "fs.not_found",
            Self::Denied => "fs.denied",
            Self::Failed => "fs.failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
}

/// What a program run from a command line can reach.
pub trait World {
    /// Writes to standard output.
    fn write(&mut self, text: &str);
    /// Writes to standard error.
    fn warn(&mut self, text: &str);
    /// The next line of standard input, without its ending.
    fn read_line(&mut self) -> Option<String>;
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
    /// Milliseconds since the start of 1970.
    fn now(&mut self) -> i64;
    /// Nanoseconds since a fixed moment.
    fn elapsed(&mut self) -> i64;
    fn sleep(&mut self, milliseconds: i64);
}

/// The target a program run from a command line runs under.
#[must_use]
pub fn target() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, "system").value("cli")
}

/// The error a failure is, as `ambient.Error` describes one.
fn error(failure: &Failure) -> Value {
    let kind = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: failure.kind.tag().to_owned(),
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
    type Handler = fn(&mut dyn World, &[Value]) -> Result<Value, String>;
    let handlers: [(&str, Handler); 16] = [
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
        ("console_read_line", |world, _| {
            Ok(optional(world.read_line()))
        }),
        ("env_args", |world, _| Ok(strings(world.args()))),
        ("env_variable", |world, args| {
            Ok(optional(world.variable(text(args, 0)?)))
        }),
        ("fs_read", |world, args| {
            let read = world.read(text(args, 0)?);
            Ok(read.map_or_else(|failure| error(&failure), Value::String))
        }),
        ("fs_write", |world, args| {
            let stored = world.store(text(args, 0)?, text(args, 1)?);
            Ok(stored.map_or_else(|failure| error(&failure), |()| Value::Null))
        }),
        ("fs_list", |world, args| {
            let listed = world.list(text(args, 0)?);
            Ok(listed.map_or_else(|failure| error(&failure), strings))
        }),
        ("fs_exists", |world, args| {
            Ok(Value::Bool(world.exists(text(args, 0)?)))
        }),
        ("fs_remove", |world, args| {
            let removed = world.remove(text(args, 0)?);
            Ok(removed.map_or_else(|failure| error(&failure), |()| Value::Null))
        }),
        ("fs_make_directory", |world, args| {
            let made = world.make_directory(text(args, 0)?);
            Ok(made.map_or_else(|failure| error(&failure), |()| Value::Null))
        }),
        ("clock_now", |world, _| Ok(Value::Int(world.now()))),
        ("clock_elapsed", |world, _| Ok(Value::Int(world.elapsed()))),
        ("clock_sleep", |world, args| {
            world.sleep(int(args, 0)?);
            Ok(Value::Null)
        }),
        // What a root handler of an ability that does not return gives
        // is what the program gives.
        ("process_exit", |_, args| Ok(Value::Int(int(args, 0)?))),
    ];
    handlers
        .into_iter()
        .fold(Natives::new(), |natives, (name, handler)| {
            let world = Rc::clone(world);
            natives.with(FAULT_PACKAGE, &format!("system.{name}"), move |args| {
                handler(&mut *world.borrow_mut(), args)
            })
        })
}
