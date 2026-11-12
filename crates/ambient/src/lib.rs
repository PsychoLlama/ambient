//! The Ambient language.
//!
//! `spec/` at the repository root is the source of truth for the design; the
//! module docs here say which page each module implements.
//!
//! A program enters as [`surface`] trees made with a [`builder`], held by a
//! [`workspace`]. An [`engine::Engine`] answers questions about it on demand,
//! keeping every stage result in a [`store`].

pub mod backend;
pub mod builder;
pub mod checker;
pub mod core;
pub mod engine;
pub mod error;
pub mod exchange;
pub mod hash;
pub mod library;
pub mod lir;
pub mod literal;
pub mod lower;
pub mod store;
pub mod surface;
pub mod symbol;
pub mod types;
pub mod value;
pub mod workspace;
