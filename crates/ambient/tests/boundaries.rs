//! Boundaries, run on every backend. See `spec/values.md` and
//! `spec/errors.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::{FAULT_PACKAGE, Fault};
use support::run;

fn outcome(
    ret: Type,
    body: Expr,
    extra: impl IntoIterator<Item = Definition>,
) -> Result<Value, Fault> {
    let main = b::function("main").returns(ret).body(body);
    run(extra.into_iter().chain([main]), &[])
}

fn evaluates(body: Expr) -> Value {
    outcome(ty::unknown(), body, []).unwrap()
}

fn overflows() -> Expr {
    b::add(b::int(i64::MAX), b::int(1))
}

/// The kind of the error `value` is.
fn kind(value: &Value) -> Value {
    match value {
        Value::Record(fields) => fields["kind"].clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn fault(tag: &str) -> Value {
    Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: tag.to_owned(),
    }
}

#[test]
fn a_boundary_is_what_its_body_gives() {
    assert_eq!(evaluates(b::boundary(b::int(1))), Value::Int(1));
}

#[test]
fn a_boundary_gives_the_fault_that_ended_its_body_as_an_error() {
    let Value::Record(error) = evaluates(b::boundary(overflows())) else {
        panic!("expected an error");
    };

    assert_eq!(error["kind"], fault("overflow"));
    assert!(matches!(error["message"], Value::String(_)));
    assert_eq!(error["cause"], Value::Null);
    assert_eq!(error["trace"], Value::List(vec![]));
}

#[test]
fn the_rest_of_the_program_goes_on_after_a_contained_fault() {
    let arms = [(pat::is(ty::int()), b::int(1)), (pat::any(), b::int(2))];
    let body = b::add(b::match_(b::boundary(overflows()), arms), b::int(40));

    assert_eq!(outcome(ty::int(), body, []), Ok(Value::Int(42)));
}

#[test]
fn a_boundary_has_the_type_of_its_body_or_an_error() {
    let error = ty::record([
        ("kind", ty::tag()),
        ("message", ty::string()),
        ("cause", ty::null()),
    ]);
    let body = || b::boundary(b::int(1));

    assert!(outcome(ty::union([ty::int(), error]), body(), []).is_ok());
}

#[test]
fn an_abort_passes_a_boundary() {
    let fail = b::ability("fail", "fail").returns(ty::never());
    let body = b::provide(
        [("fail", b::lambda([], b::int(-1)))],
        b::boundary(b::perform("fail", [])),
    );

    assert_eq!(
        outcome(ty::unknown(), body, [fail.into()]),
        Ok(Value::Int(-1))
    );
}

#[test]
fn a_boundary_contains_a_fault_in_a_task_it_started() {
    let tasks = b::all([b::int(1), overflows()]);

    assert_eq!(kind(&evaluates(b::boundary(tasks))), fault("overflow"));
}

#[test]
fn cleanups_run_before_a_boundary_has_its_fault() {
    let c = || b::name("c");
    let noted = b::cleanup(b::replace(c(), b::int(0), b::int(1)), overflows());
    let body = b::let_(
        "c",
        b::cell(ty::int(), b::int(0)),
        b::tuple([b::boundary(noted), b::read(c())]),
    );
    let Value::Tuple(members) = evaluates(body) else {
        panic!("expected a pair");
    };

    assert_eq!(kind(&members[0]), fault("overflow"));
    assert_eq!(members[1], Value::Int(1));
}

#[test]
fn a_program_that_stalls_inside_a_boundary_still_ends() {
    let c = || b::name("c");
    let waits = b::wait(c(), b::int(0));
    let body = b::let_("c", b::cell(ty::int(), b::int(0)), b::boundary(waits));

    assert_eq!(outcome(ty::unknown(), body, []), Err(Fault::Cancelled));
}
