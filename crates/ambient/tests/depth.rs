//! Calls nested too deeply, run on every backend. See "Calls" in
//! `docs/spec/backends.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::{FAULT_PACKAGE, Fault};
use support::run;

/// More calls than may nest.
const MANY: i64 = 10_000_000;

/// `fn name(n: int): int`, which counts down to zero through `next`
/// and adds one on the way back, so no call of it is its last.
fn nesting(name: &str, next: &str) -> Definition {
    let less = b::subtract(b::name("n"), b::int(1));
    let deeper = b::add(b::call(b::name(next), [less]), b::int(1));
    b::function(name)
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), b::int(0)), (pat::any(), deeper)],
        ))
}

/// `fn main(): unknown`, beside functions that nest: `deep` in itself,
/// and `left` through `through`, which ends by calling it.
fn outcome(body: Expr) -> Result<Value, Fault> {
    let through = b::function("through")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::call(b::name("left"), [b::name("n")]));
    let main = b::function("main").returns(ty::unknown()).body(body);
    let definitions = [
        nesting("deep", "deep"),
        nesting("left", "through"),
        through,
        main,
    ];
    run(definitions, &[])
}

fn deep(name: &str, n: i64) -> Expr {
    b::call(b::name(name), [b::int(n)])
}

/// What `members` is with each error in it replaced by its kind.
fn kinds(members: &Value) -> Vec<Value> {
    let Value::Tuple(members) = members else {
        panic!("expected a tuple, got {members:?}");
    };
    let kind = |member: &Value| match member {
        Value::Record(fields) => fields["kind"].clone(),
        other => other.clone(),
    };
    members.iter().map(kind).collect()
}

fn too_deep() -> Value {
    Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: "too_deep".to_owned(),
    }
}

#[test]
fn calls_nest_thousands_deep() {
    assert_eq!(outcome(deep("deep", 5_000)), Ok(Value::Int(5_000)));
    assert_eq!(outcome(deep("left", 2_000)), Ok(Value::Int(2_000)));
}

#[test]
fn calls_nested_too_deeply_are_a_fault() {
    assert_eq!(outcome(deep("deep", MANY)), Err(Fault::Deep));
}

#[test]
fn a_call_made_after_one_that_left_is_counted_too() {
    assert_eq!(outcome(deep("left", MANY)), Err(Fault::Deep));
}

#[test]
fn a_boundary_contains_the_fault_and_calls_nest_again_after_it() {
    let after = b::tuple([b::boundary(deep("deep", MANY)), deep("deep", 5_000)]);
    let expected = vec![too_deep(), Value::Int(5_000)];
    assert_eq!(kinds(&outcome(after).unwrap()), expected);
}

#[test]
fn a_task_counts_from_where_it_starts() {
    let both = b::all([
        b::add(deep("deep", 5_000), b::int(0)),
        b::boundary(deep("deep", MANY)),
    ]);
    let expected = vec![Value::Int(5_000), too_deep()];
    assert_eq!(kinds(&outcome(both).unwrap()), expected);
}
