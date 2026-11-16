//! The native side of the standard library's command-line target: what
//! `system.cli` binds each ability to. See `spec/targets.md`.
//!
//! All of it that needs no compiler is `ambient_host::host`, whose
//! names are given again here. What is added is what needs one:
//! compiling a program under the target ([`link`]), and the handlers of
//! `system.code` that pack and compile ([`compiling`]).

mod code;

use std::cell::RefCell;
use std::rc::Rc;

pub use ambient_host::host::{
    Chunk, Command, Done, Entry, Failure, Incoming, Info, Kind, Line, Listening, Ran, Request,
    Response, System, Work, World, bytes, error, invalid,
};
pub use code::{Linked, link, offer as compiling};

use crate::backend::native::Natives;
use crate::pack::Pack;

/// The native code for every handler of the command-line target, over
/// one world, for a program whose trees are `trees`: see [`link`].
#[must_use]
pub fn natives(world: &Rc<RefCell<dyn World>>, trees: Option<Pack>) -> Natives {
    compiling(ambient_host::host::natives(world), trees)
}
