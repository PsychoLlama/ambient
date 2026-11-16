//! Loading what the native backend produced into this process, and
//! running it. The backend itself, which needs a compiler, is
//! `ambient::backend::native`.

mod externs;
mod host;
mod load;
mod unit;
mod values;
mod waits;

pub use externs::Externs;
pub use host::{
    ANSWERING, NativeFn, Natives, Nowhere, Observation, Offered, Running, UNCALLABLE, UNDECLARED,
    UNDECLARED_ABILITY, UNWOUND, bound,
};
pub use load::Program;
pub use values::{Host, Inside, encode};
pub use waits::{Answer, Answers, Waits};

/// What names the native backend ([`Running::backend`], `Backend::name`):
/// the backend and the machine, since code for one does not run on
/// another.
pub const NAME: &str = concat!("native-", env!("AMBIENT_TARGET"));
