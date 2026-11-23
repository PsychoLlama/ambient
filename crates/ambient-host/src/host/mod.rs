//! The native side of the standard library's command-line target: what
//! `system.cli` binds each ability to. See `docs/spec/targets.md`.
//!
//! Everything a program can do to the world goes through a [`World`], so
//! the same handlers serve a real process ([`System`]) and a test that
//! scripts what the world holds.

mod code;
mod commands;
mod crypto;
mod exchange;
mod files;
mod http;
mod linked;
mod net;
mod network;
mod owned;
mod pending;
mod pinned;
mod plain;
mod queue;
mod reaching;
mod signals;
mod sockets;
mod streams;
mod system;
mod trust;
mod waits;
mod watching;
mod zone;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub use code::{invalid, names_native};
pub use commands::Command;
pub use files::{Entry, Info};
pub use http::{Incoming, Listening, Request, Response, Streamed};
pub use linked::{GROUPS, Group, Linked, group};
pub use net::{Address, Datagram, Held, Open, Opened, Reach, Reached};
pub use signals::Signal;
pub use system::System;
pub use trust::Trust;
pub use waits::{Chunk, Done, Line, Work};
pub use watching::{Change, Changed};

use crate::native::Natives;
use crate::value::Value;

/// The name of the `system` package and its ID, which no other package
/// may take. Its modules are the compiler's (`ambient::system`).
pub const PACKAGE: &str = "system";

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
    /// The connection could not be secured: the server did not show
    /// it is who it was asked for, to an authority that is trusted.
    Untrusted,
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
            Self::Untrusted => "untrusted",
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
    pub out: Vec<u8>,
    pub err: Vec<u8>,
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
    /// Every environment variable and its value, in order of name.
    fn variables(&self) -> Vec<(String, String)>;
    /// What a file holds.
    ///
    /// # Errors
    ///
    /// The file cannot be read.
    fn read(&mut self, path: &str) -> Result<Vec<u8>, Failure>;
    /// Makes a file hold `bytes`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    fn store(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure>;
    /// Makes a file hold `bytes`, as one that can be run.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    fn install(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure>;
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
    /// What is at a path.
    ///
    /// # Errors
    ///
    /// Nothing is there, or it cannot be looked at.
    fn info(&mut self, path: &str) -> Result<Info, Failure>;
    /// Makes a file hold `bytes`, as [`Self::store`] does, that only
    /// its owner may read or write. It is made so, or made so before
    /// anything is written if it was there: never open to others with
    /// the bytes in it.
    ///
    /// # Errors
    ///
    /// The file cannot be written, or cannot be kept from others.
    fn store_private(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure>;
    /// Adds `bytes` to the end of a file, making it if it is not there.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure>;
    /// Moves what is at one path to another.
    ///
    /// # Errors
    ///
    /// Nothing is there, or it cannot be moved.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Failure>;
    /// Makes a file hold what another holds.
    ///
    /// # Errors
    ///
    /// The one cannot be read, or the other written.
    fn copy(&mut self, from: &str, to: &str) -> Result<(), Failure>;
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
    /// The milliseconds the machine's time zone is ahead of UTC at a
    /// moment, which is milliseconds since the start of 1970.
    fn offset(&mut self, moment: i64) -> i64;
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
    /// Lets go of a request a server accepted, if it was not answered:
    /// nothing can answer it any more.
    fn abandon(&mut self, incoming: i64);
    /// Opens a listener or a socket.
    ///
    /// # Errors
    ///
    /// It cannot be opened there.
    fn open(&mut self, what: Open) -> Result<Opened, Failure>;
    /// Lets go of a stream, a listener, or a socket: whatever waits on
    /// it is given nothing.
    fn shut(&mut self, held: Held);
    /// Begins work that takes time. What comes of it is given by
    /// [`World::next`], under `ticket`.
    fn begin(&mut self, ticket: u64, work: Work);
    /// What came of the work that is over soonest, waited for if none
    /// is yet. None if no work is under way.
    fn next(&mut self) -> Option<(u64, Done)>;
    /// Stops the work of `ticket`, if it can be stopped: nothing waits
    /// for it any more.
    fn forsake(&mut self, ticket: u64);

    /// Hands over what threads of native bundles post for the
    /// program: see [`crate::native::Waits::attend`].
    fn attend(&mut self, _posts: std::sync::Arc<crate::native::Posts>) {}
}

/// The error a failure is, as `ambient.Error` describes one: its kind
/// is the tag for it of `module`, whose ability was refused.
#[must_use]
pub fn error(module: &str, failure: &Failure) -> Value {
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

/// The bytes a handler was given at `position`.
///
/// # Errors
///
/// Fails if what is there is not bytes.
pub fn bytes(args: &[Value], position: usize) -> Result<&[u8], String> {
    match args.get(position) {
        Some(Value::Bytes(bytes)) => Ok(bytes),
        _ => Err(format!("argument {position} is not bytes")),
    }
}

fn int(args: &[Value], position: usize) -> Result<i64, String> {
    match args.get(position) {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(format!("argument {position} is not an integer")),
    }
}

/// What a command gave, as `system.process.Output` describes it.
fn output(ran: Ran) -> Value {
    let fields = [
        ("code", Value::Int(ran.code)),
        ("out", Value::Bytes(ran.out)),
        ("err", Value::Bytes(ran.err)),
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

/// `natives`, with the extern functions of `system` that are no
/// abilities: what is only computed, as a hash is. They reach no
/// world, so a test and a build have them as any program does. See
/// "The system package" in `docs/spec/targets.md`.
#[must_use]
pub fn computing(natives: Natives) -> Natives {
    exchange::offer(crypto::offer(natives))
}

/// The native code for every handler of the command-line target that
/// needs no compiler, over one world. Packing a function and compiling
/// one are the compiler's to add (`ambient::host::natives`).
#[must_use]
pub fn natives(world: &Rc<RefCell<dyn World>>) -> Natives {
    natives_linked(world, &Linked::whole())
}

/// [`natives`], for a process that was linked with `linked`: it has
/// the handlers of the groups the link named, and no other. Whatever
/// native code the command-line target gains is offered by a group.
#[must_use]
pub fn natives_linked(world: &Rc<RefCell<dyn World>>, linked: &Linked) -> Natives {
    let outcomes = Rc::new(RefCell::new(waits::Outcomes(Rc::clone(world))));
    let natives = Natives::new().answered_by(outcomes);
    let natives = linked
        .natives
        .iter()
        .fold(natives, |natives, offer| offer(natives));
    linked
        .offers
        .iter()
        .fold(natives, |natives, offer| offer.over(natives, world))
}
