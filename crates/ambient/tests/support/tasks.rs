//! Definitions that programs with tasks and cells are written with.

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr};

/// `first`, then `second`.
pub fn then(first: Expr, second: Expr) -> Expr {
    b::let_("unused", first, second)
}

/// `fn until(c: Cell<int>, want: int): int`, which waits until the cell
/// holds `want` and then gives it.
pub fn until() -> Definition {
    let again = b::call(b::name("until"), [b::name("c"), b::name("want")]);
    let waits = then(b::wait(b::name("c"), b::name("seen")), again);
    let arms = [(pat::boolean(true), b::name("seen")), (pat::any(), waits)];
    let held = b::match_(b::equal(b::name("seen"), b::name("want")), arms);
    b::function("until")
        .param("c", ty::cell(ty::int()))
        .param("want", ty::int())
        .returns(ty::int())
        .body(b::let_("seen", b::read(b::name("c")), held))
}

/// `fn set(c: Cell<int>, value: int): null`, which tries until it has put
/// the value in.
pub fn set() -> Definition {
    let again = b::call(b::name("set"), [b::name("c"), b::name("value")]);
    let replaced = b::replace(b::name("c"), b::read(b::name("c")), b::name("value"));
    b::function("set")
        .param("c", ty::cell(ty::int()))
        .param("value", ty::int())
        .returns(ty::null())
        .body(b::match_(
            replaced,
            [(pat::boolean(true), b::null()), (pat::any(), again)],
        ))
}

/// `fn note(log: Cell<int>, entry: int): null`, which adds to a log.
/// Lists cannot be appended to yet, so the log is a number and an entry
/// is its next digit.
pub fn note() -> Definition {
    let log = || b::name("log");
    let shifted = b::add(b::multiply(b::read(log()), b::int(10)), b::name("entry"));
    b::function("note")
        .param("log", ty::cell(ty::int()))
        .param("entry", ty::int())
        .returns(ty::null())
        .body(then(b::replace(log(), b::read(log()), shifted), b::null()))
}
