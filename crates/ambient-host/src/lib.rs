//! What a host needs of Ambient to load a compiled program and run it,
//! and nothing that compiles one.
//!
//! The compiler (`ambient`) is built on this and gives every module
//! here again under the path it has there, so `ambient::core` is
//! [`core`]. What only a host needs is here too: a program's [`parts`]
//! and how they are laid out ([`statics`]), loading them into this
//! process ([`native`]), the native side of the `system` package
//! ([`host`]), and starting a program and reporting how it ended
//! ([`launch`]).

pub mod core;
mod deep;
pub mod exchange;
pub mod hash;
pub mod host;
mod identity;
pub mod launch;
pub mod literal;
pub mod native;
pub mod pack;
pub mod parts;
pub mod stack;
pub mod statics;
pub mod symbol;
pub mod value;

pub use deep::deep;
