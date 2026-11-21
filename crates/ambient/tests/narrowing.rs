//! What a match tests as a program runs: no more than tells the values
//! of an arm from the rest of its subject's type. See "Narrowing" in
//! `docs/spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use support::run;

/// Whether `value` is taken by the arm for `held`, in a match on a
/// value that is of `held` or of `other`.
fn taken(held: &Type, other: Type, value: Expr) -> bool {
    let arms = [
        (pat::is(held.clone()), b::boolean(true)),
        (pat::is(other.clone()), b::boolean(false)),
    ];
    let pick = b::function("pick")
        .param("x", ty::union([held.clone(), other]))
        .returns(ty::bool())
        .body(b::match_(b::name("x"), arms));
    let main = b::function("main")
        .returns(ty::bool())
        .body(b::call(b::name("pick"), [value]));
    run([pick, main], &[]).unwrap() == Value::Bool(true)
}

/// `fn(): int`, and a function whose own type is within it: it gives
/// the literal `1`.
fn gives_int() -> (Type, Expr) {
    (ty::function([], ty::int()), b::lambda([], b::int(1)))
}

#[test]
fn a_function_is_matched_as_a_type_its_own_is_within() {
    let (function, one) = gives_int();
    assert!(taken(&function, ty::null(), one));
}

#[test]
fn a_record_or_a_list_of_such_functions_is_matched_by_its_shape() {
    let (function, one) = gives_int();
    let record = ty::record([("give", function)]);
    let held = || b::record([("give", one.clone())]);
    assert!(taken(&record, ty::null(), held()));
    assert!(taken(&ty::list(record), ty::string(), b::list([held()])));
}

#[test]
fn a_function_is_told_from_what_is_none_inside_a_record() {
    let (function, one) = gives_int();
    let of = |held| ty::record([("give", held)]);
    let held = b::record([("give", one)]);
    assert!(taken(&of(function), of(ty::int()), held));
}
