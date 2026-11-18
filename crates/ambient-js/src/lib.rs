//! The tests of the JavaScript target (`packages/js`): what builds an
//! Ambient program for a JavaScript host, and the host that loads and
//! runs it. See "JavaScript hosts" in `spec/targets.md`.
//!
//! The target is an Ambient package and its host is JavaScript, so
//! nothing of either is Rust. This crate builds a program with them
//! and runs the JavaScript tests, so that the one command that runs
//! every test runs these too.
