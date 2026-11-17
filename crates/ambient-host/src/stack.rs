//! The stack a program runs on. See "Calls" in `spec/backends.md`.
//!
//! Calls nest no deeper than the runtime counts
//! ([`ambient_runtime::DEPTH`]), so a host that gives a program room
//! for that never sees one overflow.

/// The stack a program may take, in bytes: what the deepest calls keep,
/// many times over, since a frame holds more than what is counted.
///
/// Measured, not guessed: in a build that is not optimized, the
/// deepest calls the tests make overflow 16 MB and fit in 32, where a
/// call is made after one that left and the runtime's own frames
/// stand between. So this is twice what is needed, and no less will
/// do while those frames are not counted.
pub const ROOM: usize = 64 << 20;

/// Runs `work`, which runs a program, with [`ROOM`] and room for the
/// host under it.
pub fn roomy<R>(work: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(ROOM + (ROOM >> 4), 2 * ROOM, work)
}
