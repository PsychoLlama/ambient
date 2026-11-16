//! The stack that work on a tree takes.

/// Runs what works on a tree or a lowered function, on a stack with
/// room for it.
///
/// A walk recurses once per level. A core tree nests no deeper than
/// [`crate::core::DEPTH`]; lowering binds every intermediate value with
/// a `Let` whose body is what comes after, so a lowered function nests as
/// deeply as it is long, and making, translating, and dropping one all
/// recurse that deeply.
pub fn deep<R>(work: impl FnOnce() -> R) -> R {
    /// Room for a function of some hundreds of thousands of operations,
    /// which is what is left when work begins: a new stack is twice it.
    const ROOM: usize = 256 << 20;
    stacker::maybe_grow(ROOM, 2 * ROOM, work)
}
