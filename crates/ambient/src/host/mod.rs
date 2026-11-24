//! The native side of the standard library's command-line target: what
//! `system.cli` binds each ability to. See `docs/spec/targets.md`.
//!
//! All of it that needs no compiler is `ambient_host::host`, whose
//! names are given again here. What is added is what needs one:
//! compiling a program under the target ([`link`]), and the handlers of
//! `system.code` that pack and compile ([`compiling`]).

mod code;

use std::cell::RefCell;
use std::rc::Rc;

pub use ambient_host::host::{
    Address, Change, Changed, Chunk, Command, Datagram, Done, Entry, Failure, GROUPS, Group, Held,
    Incoming, Info, Kind, Line, Linked as Groups, Listening, Open, Opened, Ran, Reach, Reached,
    Request, Response, Signal, Streamed, System, Trust, Work, World, bytes, error, group, invalid,
    natives_linked, pure,
};
pub use code::{Linked, compiles, link, offer as compiling};

use crate::backend::native::Natives;
use crate::pack::Pack;

/// The native code for every handler of the command-line target, over
/// one world, for a program whose trees are `trees`: see [`link`].
#[must_use]
pub fn natives(world: &Rc<RefCell<dyn World>>, trees: Option<Pack>) -> Natives {
    compiling(ambient_host::host::natives(world), trees)
}

/// The name of the compiler's group, as the `main` of a link names it:
/// a program whose parts pack or compile code ([`compiles`]) is linked
/// with the compiler, and no other is.
pub const GROUP: &str = "amb_system_compiling";

/// Links the compiler: the handlers of `system.code` that need it.
#[unsafe(no_mangle)]
pub extern "C" fn amb_system_compiling(linked: &mut Groups) {
    linked.compile_with(compiling);
}
