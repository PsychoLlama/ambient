//! The standard library. See `spec/modules.md` and `spec/errors.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::library;
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use support::{run_in, workspace};

/// What `main`, with this body, gives in a workspace that has the
/// standard library.
fn evaluates(ret: Type, body: Expr) -> Value {
    let main = b::function("main").returns(ret).body(body);
    let mut workspace = workspace([("app", "main", b::module().define(main))]);
    library::install(&mut workspace).unwrap();
    run_in(&workspace).unwrap()
}

/// Which of `types`, by position, `value` is; or `-1`, if it is none of
/// them.
fn which<const N: usize>(value: Expr, types: [&str; N]) -> Expr {
    let arms = (0..)
        .zip(types)
        .map(|(position, name)| (pat::is(ty::named(name)), b::int(position)));
    b::match_(value, arms.chain([(pat::any(), b::int(-1))]))
}

#[test]
fn a_program_names_the_kind_of_a_fault() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));
    let kind = b::match_(
        b::boundary(overflow),
        [
            (
                pat::bind_as("error", pat::is(ty::named("Error"))),
                b::field(b::name("error"), "kind"),
            ),
            (pat::any(), b::tag("none")),
        ],
    );

    assert_eq!(
        evaluates(ty::int(), which(kind, ["NoMatch", "Overflow"])),
        Value::Int(1)
    );
}

#[test]
fn a_program_names_the_limit_a_budget_ran_out_of() {
    // A sum may need room for its result, and there is none.
    let limit = b::budget(b::int(1000), b::int(0), b::add(b::int(1), b::int(1)));

    assert_eq!(
        evaluates(ty::int(), which(limit, ["Steps", "Allocation"])),
        Value::Int(1)
    );
}

#[test]
fn a_tag_of_a_program_is_not_one_of_the_language() {
    // Widened, so that the checker does not know which tag it is.
    let own = b::cast(b::tag("overflow"), ty::tag(), b::tag("other"));

    assert_eq!(
        evaluates(ty::int(), which(own, ["Overflow"])),
        Value::Int(-1)
    );
}
