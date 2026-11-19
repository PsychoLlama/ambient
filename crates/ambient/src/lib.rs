//! The Ambient language.
//!
//! `docs/spec/` at the repository root is the source of truth for the design; the
//! module docs here say which page each module implements.
//!
//! A program enters as [`surface`] trees made with a [`builder`], held by a
//! [`workspace`]. An [`engine::Engine`] answers questions about it on demand,
//! keeping every stage result in a [`store`].
//!
//! What a host needs to load and run a compiled program is a crate of
//! its own, `ambient-host`, which has no compiler in it. Its modules
//! are given again here under the paths they have always had.

pub use ambient_host::{core, exchange, hash, literal, pack, symbol, value};

pub mod backend;
pub mod builder;
pub mod checker;
pub mod cli;
pub mod engine;
pub mod error;
pub mod host;
pub mod library;
pub mod lir;
pub mod lower;
pub mod project;
pub mod store;
pub mod surface;
pub mod syntax;
pub mod system;
pub mod types;
pub mod verify;
pub mod workspace;
