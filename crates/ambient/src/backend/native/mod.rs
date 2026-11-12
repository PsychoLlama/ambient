//! The native backend: Cranelift, and loading what it produced into this
//! process.

mod emit;
mod load;

pub use emit::Native;
pub use load::{Host, Program, encode};
