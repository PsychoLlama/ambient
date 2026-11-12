//! Programs, run on every backend. See `spec/backends.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::run;

fn main(ret: Type, body: Expr) -> Definition {
    b::function("main").returns(ret).body(body)
}

fn evaluates(ret: Type, body: Expr) -> Value {
    run([main(ret, body)], &[]).unwrap()
}

/// `fn factorial(n: int): int`
fn factorial() -> Definition {
    let recurse = b::call(b::name("factorial"), [b::subtract(b::name("n"), b::int(1))]);
    b::function("factorial")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [
                (pat::int(0), b::int(1)),
                (pat::any(), b::multiply(b::name("n"), recurse)),
            ],
        ))
}

#[test]
fn immediates() {
    assert_eq!(evaluates(ty::null(), b::null()), Value::Null);
    assert_eq!(evaluates(ty::bool(), b::boolean(true)), Value::Bool(true));
    assert_eq!(evaluates(ty::int(), b::int(-7)), Value::Int(-7));
}

#[test]
fn integers_of_every_size() {
    assert_eq!(evaluates(ty::int(), b::int(i64::MAX)), Value::Int(i64::MAX));
    assert_eq!(evaluates(ty::int(), b::int(i64::MIN)), Value::Int(i64::MIN));
    // Past what fits in a word beside its tag, and back.
    let big = b::add(b::int(1 << 62), b::int(1 << 61));
    assert_eq!(
        evaluates(ty::int(), big.clone()),
        Value::Int((1 << 62) + (1 << 61))
    );
    assert_eq!(
        evaluates(ty::int(), b::subtract(big, b::int(1 << 62))),
        Value::Int(1 << 61)
    );
}

#[test]
fn arithmetic() {
    let body = b::add(b::int(1), b::multiply(b::int(2), b::int(3)));
    assert_eq!(evaluates(ty::int(), body), Value::Int(7));
    assert_eq!(
        evaluates(ty::bool(), b::less(b::int(1), b::int(2))),
        Value::Bool(true)
    );
    let body = b::multiply(b::float(1.5), b::add(b::float(1.0), b::float(1.0)));
    assert_eq!(evaluates(ty::float(), body), Value::Float(3.0));
}

#[test]
fn equality_is_structural() {
    let equal = |left, right| evaluates(ty::bool(), b::equal(left, right));

    assert_eq!(equal(b::int(1), b::int(1)), Value::Bool(true));
    assert_eq!(equal(b::int(i64::MAX), b::int(i64::MAX)), Value::Bool(true));
    assert_eq!(equal(b::float(0.5), b::float(0.5)), Value::Bool(true));
    assert_eq!(equal(b::int(1), b::float(1.0)), Value::Bool(false));
    assert_eq!(equal(b::null(), b::boolean(false)), Value::Bool(false));
}

#[test]
fn bindings_and_match() {
    let body = b::let_(
        "n",
        b::int(4),
        b::match_(
            b::less(b::name("n"), b::int(10)),
            [
                (pat::boolean(true), b::add(b::name("n"), b::int(1))),
                (pat::boolean(false), b::int(0)),
            ],
        ),
    );
    assert_eq!(evaluates(ty::int(), body), Value::Int(5));

    let bound = b::match_(
        b::int(3),
        [(pat::bind("x"), b::multiply(b::name("x"), b::name("x")))],
    );
    assert_eq!(evaluates(ty::int(), bound), Value::Int(9));
}

#[test]
fn calls_and_recursion() {
    let main = main(ty::int(), b::call(b::name("factorial"), [b::int(20)]));

    assert_eq!(
        run([factorial(), main], &[]).unwrap(),
        Value::Int(2_432_902_008_176_640_000)
    );
}

#[test]
fn arguments_from_the_host() {
    let main = b::function("main")
        .param("a", ty::int())
        .param("b", ty::int())
        .returns(ty::int())
        .body(b::subtract(b::name("a"), b::name("b")));

    assert_eq!(run([main], &[10, 3]).unwrap(), Value::Int(7));
}

#[test]
fn overflow_is_a_fault() {
    let main = main(ty::int(), b::call(b::name("factorial"), [b::int(21)]));
    assert_eq!(run([factorial(), main], &[]), Err(Fault::Overflow));

    let body = b::add(b::int(i64::MAX), b::int(1));
    assert_eq!(
        run([self::main(ty::int(), body)], &[]),
        Err(Fault::Overflow)
    );
}

#[test]
fn a_fault_unwinds_through_bindings() {
    // The fault happens while two locals are live, in a callee.
    let fails = b::function("fails")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::let_(
            "big",
            b::int(i64::MAX),
            b::add(b::name("big"), b::name("n")),
        ));
    let body = b::let_(
        "kept",
        b::add(b::int(i64::MAX), b::int(0)),
        b::add(b::call(b::name("fails"), [b::int(1)]), b::name("kept")),
    );

    assert_eq!(
        run([fails, main(ty::int(), body)], &[]),
        Err(Fault::Overflow)
    );
}
