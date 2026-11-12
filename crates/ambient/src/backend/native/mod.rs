//! The native backend: Cranelift, and loading what it produced into this
//! process.

mod emit;
mod externs;
mod host;
mod load;
mod unit;

pub use emit::Native;
pub use externs::Externs;
pub use host::{Host, Inside, NativeFn, Natives, Nowhere, Observation, Offered, Running, encode};
pub use load::Program;
