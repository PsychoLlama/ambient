//! The stack a program runs on. See "Calls" in `spec/backends.md`.
//!
//! Calls nest no deeper than the runtime counts
//! ([`ambient_runtime::DEPTH`]), so a host that gives a program room
//! for that never sees one overflow.

/// The stack a program may take, in bytes: what the deepest calls keep,
/// several times over, since a frame holds more than what is counted.
///
/// Measured, not guessed: the deepest calls the tests make overflow
/// 4 MB and fit in 8 in a build that is not optimized, and fit in 2 in
/// one that is. So this is twice what the larger needs. Every frame
/// between two calls is counted, the runtime's own among them
/// (`ambient_runtime::BOUNCED`), which is what keeps that true.
pub const ROOM: usize = 16 << 20;

/// Runs `work`, which runs a program, with [`ROOM`] and room for the
/// host under it.
pub fn roomy<R>(work: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(ROOM + (ROOM >> 4), 2 * ROOM, work)
}
