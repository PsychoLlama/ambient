//! The native backend: Cranelift, and loading what it produced into this
//! process.

mod emit;
mod host;
mod load;

pub use emit::Native;
pub use host::{Host, Inside, NativeFn, Natives, Observation, Offered, encode};
pub use load::Program;
