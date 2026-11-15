//! The native backend: Cranelift, and loading what it produced into this
//! process.

mod emit;
mod externs;
mod host;
mod load;
mod unit;
mod values;
mod waits;

pub use emit::Native;
pub use externs::Externs;
pub use host::{NativeFn, Natives, Nowhere, Observation, Offered, Running, UNDECLARED};
pub use load::Program;
pub use values::{Host, Inside, encode};
pub use waits::{Answer, Answers, Waits};
