//! The JavaScript host of an Ambient program: what loads a WebAssembly
//! module built with `ambient build --module` and runs it. See
//! "JavaScript hosts" in `spec/targets.md`.
//!
//! The host is JavaScript (`host.js`), and knows of no browser and no
//! Node: a target for either builds on it, and brings what its world
//! offers. This crate holds the text for whatever writes it beside a
//! module, and the tests that run it.

/// The host: one module, with no dependencies.
pub const HOST: &str = include_str!("../host.js");
