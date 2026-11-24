//! The launcher (`crates/ambient-launcher`) as an archive the machine's
//! linker joins with what a program reaches. See "Native code" in
//! `docs/spec/linking.md`.
//!
//! It is a crate of its own so that the compiler does not wait for it:
//! the build script builds the launcher, which holds the compiler, and
//! only the command and the tests that link depend on what it made.

/// The archive, compressed.
pub const PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/launcher.a.gz"));

/// The libraries of the system the archive is linked with, as the
/// compiler that made it names them to a linker.
pub const LIBRARIES: &str = include_str!(concat!(env!("OUT_DIR"), "/launcher.libraries"));
