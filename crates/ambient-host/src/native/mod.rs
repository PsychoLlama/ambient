//! Loading what the native backend produced into this process, and
//! running it. The backend itself, which needs a compiler, is
//! `ambient::backend::native`.

mod bundle;
mod externs;
mod host;
mod load;
mod natives;
mod stacks;
mod unit;
mod values;
mod waits;

pub use bundle::{
    Bundled, Calling, PLACED, amb_bundle_call, amb_bundle_fail, amb_bundle_handler,
    amb_bundle_later,
};
pub use externs::Externs;
pub use host::{
    ANSWERING, Nowhere, Questions, Running, UNASKED, UNCALLABLE, UNDECLARED, UNDECLARED_ABILITY,
    UNWOUND, bound,
};
pub use load::Program;
pub use natives::{NativeFn, Natives, Observation, Offered};
pub use values::{Host, Inside, encode};
pub use waits::{Answer, Answers, Waits};

/// The machine native code here is for: what a native bundle says it
/// is for, to be linked into a program built here.
pub const MACHINE: &str = env!("AMBIENT_TARGET");

/// What names the native backend ([`Running::backend`], `Backend::name`):
/// the backend and the machine, since code for one does not run on
/// another.
pub const NAME: &str = concat!("native-", env!("AMBIENT_TARGET"));
