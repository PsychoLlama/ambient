//! The native backend: Cranelift. Loading what it produced into this
//! process is `ambient_host::native`, whose names are given again
//! here.

mod emit;

pub use ambient_host::native::{
    ANSWERING, Answer, Answers, Externs, Host, Inside, NAME, NativeFn, Natives, Nowhere,
    Observation, Offered, Program, Running, UNCALLABLE, UNDECLARED, UNDECLARED_ABILITY, UNWOUND,
    Waits, bound, encode,
};
pub use emit::Native;
