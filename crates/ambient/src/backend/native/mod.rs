//! The native backend: Cranelift. Loading what it produced into this
//! process is `ambient_host::native`, whose names are given again
//! here.

mod emit;

pub use ambient_host::native::{
    ANSWERING, Answer, Answers, Bundled, Calling, Externs, Host, Inside, MACHINE, NAME, NativeFn,
    Natives, Nowhere, Observation, Offered, PLACED, Program, Questions, Running, UNCALLABLE,
    UNDECLARED, UNDECLARED_ABILITY, UNWOUND, Waits, amb_bundle_call, amb_bundle_fail,
    amb_bundle_handler, amb_bundle_later, bound, encode,
};
pub use emit::Native;
