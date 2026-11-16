//! The stack a program runs on. See "Calls" in `spec/backends.md`.
//!
//! Calls nest no deeper than the runtime counts
//! ([`ambient_runtime::DEPTH`]), so a host that gives a program room
//! for that never sees one overflow.

/// The stack a program may take, in bytes: what the deepest calls keep,
/// many times over, since a frame holds more than what is counted.
pub const ROOM: usize = 64 << 20;

/// Runs `work`, which runs a program, with [`ROOM`] and room for the
/// host under it.
pub fn roomy<R>(work: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(ROOM + (ROOM >> 4), 2 * ROOM, work)
}
