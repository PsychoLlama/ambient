//! Operations: what the language itself does with numbers, strings, and
//! lists. See `spec/operations.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use ambient_runtime::{FAULT_PACKAGE, Fault};
use support::{Outcome, run, value, workspace};

/// `fn main(): ret`, which is `body`.
fn main(ret: Type, body: Expr) -> ambient::surface::Definition {
    b::function("main").returns(ret).body(body)
}

/// What `body` evaluates to, or the fault that ends it.
fn outcome(body: Expr) -> Outcome {
    run([main(ty::unknown(), body)], &[])
}

/// What each of `bodies` evaluates to, in one program.
fn each(bodies: impl IntoIterator<Item = Expr>) -> Vec<Value> {
    match &mut outcome(b::list(bodies)) {
        Ok(Value::List(values)) => std::mem::take(values),
        other => panic!("expected a list, got {other:?}"),
    }
}

/// Whether `body` checks as a `ret`.
fn checks(ret: Type, body: Expr) -> Result<(), Error> {
    let workspace = workspace([("app", "main", b::module().define(main(ret, body)))]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.map(|_| ())
}

fn ints<const N: usize>(values: [i64; N]) -> Value {
    Value::List(values.into_iter().map(Value::Int).collect())
}

fn int_list<const N: usize>(values: [i64; N]) -> Expr {
    b::list(values.map(b::int))
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

// Numbers

#[test]
fn integers_divide_toward_zero() {
    let quotients = [
        b::divide(b::int(7), b::int(2)),
        b::divide(b::int(-7), b::int(2)),
    ];
    let remainders = [
        b::remainder(b::int(7), b::int(2)),
        b::remainder(b::int(-7), b::int(2)),
    ];

    assert_eq!(each(quotients), [Value::Int(3), Value::Int(-3)]);
    assert_eq!(each(remainders), [Value::Int(1), Value::Int(-1)]);
}

#[test]
fn dividing_an_integer_by_zero_is_a_fault() {
    let least = b::int(i64::MIN);

    assert_eq!(
        outcome(b::divide(b::int(1), b::int(0))),
        Err(Fault::DivideByZero)
    );
    assert_eq!(
        outcome(b::remainder(b::int(1), b::int(0))),
        Err(Fault::DivideByZero)
    );
    assert_eq!(outcome(b::divide(least, b::int(-1))), Err(Fault::Overflow));
}

#[test]
fn floats_divide_without_faulting() {
    let quotients = [
        b::divide(b::float(1.0), b::float(4.0)),
        b::divide(b::float(1.0), b::float(0.0)),
    ];

    assert_eq!(
        each(quotients),
        [Value::Float(0.25), Value::Float(f64::INFINITY)]
    );
}

#[test]
fn an_integer_and_a_float_do_not_divide() {
    let result = checks(ty::unknown(), b::divide(b::int(1), b::float(2.0)));

    assert!(matches!(result, Err(Error::Operands { .. })));
}

// Strings

#[test]
fn strings_are_ordered_by_their_bytes() {
    let less = |left, right| b::less(b::string(left), b::string(right));
    let ordered = each([
        less("a", "b"),
        less("b", "a"),
        less("a", "ab"),
        less("Z", "a"),
    ]);

    assert_eq!(ordered, [true, false, true, true].map(Value::Bool));
}

#[test]
fn a_string_is_as_long_as_its_bytes() {
    let lengths = each([b::length(b::string("")), b::length(b::string("héllo"))]);

    assert_eq!(lengths, [Value::Int(0), Value::Int(6)]);
}

#[test]
fn strings_join() {
    let joined = b::join(b::string("ab"), b::join(b::string(""), b::string("c")));

    assert_eq!(outcome(joined), Ok(text("abc")));
}

#[test]
fn a_slice_of_a_string_is_what_lies_between_two_positions_or_a_miss() {
    // Every pair of positions, past the end and before the start too.
    let whole = "aé😀z";
    let positions = || -1..=i64::try_from(whole.len()).unwrap() + 1;
    let pairs = || positions().flat_map(|start| positions().map(move |end| (start, end)));
    let slice = |(start, end)| b::slice(b::string(whole), b::int(start), b::int(end));
    let expected = |(start, end): (i64, i64)| {
        let range = usize::try_from(start).ok()?..usize::try_from(end).ok()?;
        whole.get(range).map(text)
    };

    let expected: Vec<Value> = pairs()
        .map(|pair| expected(pair).unwrap_or(Value::Null))
        .collect();
    assert_eq!(each(pairs().map(slice)), expected);
}

// Lists

#[test]
fn a_list_is_as_long_as_its_members() {
    let lengths = each([b::length(b::list([])), b::length(int_list([4, 5, 6]))]);

    assert_eq!(lengths, [Value::Int(0), Value::Int(3)]);
}

#[test]
fn getting_a_member_gives_it_or_misses() {
    let get = |position| b::get(int_list([4, 5, 6]), b::int(position));

    let got = each([get(0), get(2), get(3), get(-1)]);
    assert_eq!(
        got,
        [Value::Int(4), Value::Int(6), Value::Null, Value::Null]
    );
    // The miss is in the type.
    assert!(checks(ty::int(), get(0)).is_err());
    assert!(checks(ty::union([ty::int(), ty::null()]), get(0)).is_ok());
}

#[test]
fn pushing_gives_a_list_of_what_was_there_and_what_was_pushed() {
    let pushed = || b::push(int_list([1, 2]), b::string("three"));
    let either = ty::list(ty::union([ty::int(), ty::string()]));

    let expected = Value::List(vec![Value::Int(1), Value::Int(2), text("three")]);
    assert_eq!(outcome(pushed()), Ok(expected));
    assert!(checks(either, pushed()).is_ok());
    assert!(checks(ty::list(ty::int()), pushed()).is_err());
}

#[test]
fn pushing_leaves_the_list_it_was_given_as_it_was() {
    let both = b::tuple([b::push(b::name("list"), b::int(3)), b::name("list")]);
    let body = b::let_("list", int_list([1, 2]), both);

    let expected = Value::Tuple(vec![ints([1, 2, 3]), ints([1, 2])]);
    assert_eq!(outcome(body), Ok(expected));
}

#[test]
fn lists_join() {
    let joined = b::join(int_list([1]), b::join(b::list([]), int_list([2, 3])));

    assert_eq!(outcome(joined), Ok(ints([1, 2, 3])));
}

#[test]
fn a_slice_of_a_list_is_what_lies_between_two_positions_or_a_miss() {
    let slice = |start, end| b::slice(int_list([4, 5, 6]), b::int(start), b::int(end));

    let slices = each([
        slice(0, 3),
        slice(1, 2),
        slice(2, 2),
        slice(2, 1),
        slice(0, 4),
        slice(-1, 2),
    ]);
    let expected = [
        ints([4, 5, 6]),
        ints([5]),
        ints([]),
        Value::Null,
        Value::Null,
        Value::Null,
    ];
    assert_eq!(slices, expected);
}

#[test]
fn a_tuple_is_a_list_to_every_list_operation() {
    let pair = || b::tuple([b::int(1), b::string("two")]);
    let results = each([
        b::length(pair()),
        b::get(pair(), b::int(1)),
        b::push(pair(), b::int(3)),
    ]);

    let pushed = Value::List(vec![Value::Int(1), text("two"), Value::Int(3)]);
    assert_eq!(results, [Value::Int(2), text("two"), pushed]);
}

#[test]
fn only_strings_and_lists_have_a_length() {
    let result = checks(ty::int(), b::length(b::int(1)));

    assert!(matches!(result, Err(Error::Operands { .. })));
}

// Budgets

#[test]
fn an_operation_that_copies_is_charged_for_what_it_copies() {
    let long = || int_list([1, 2, 3, 4, 5, 6, 7, 8]);
    let joined = |allocation| {
        let body = b::join(long(), long());
        outcome(b::budget(b::int(100), b::int(allocation), body))
    };
    let exhausted = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: "allocation".to_owned(),
    };

    // Two lists of eight, each an object of nine words, and a copy of
    // each.
    let whole = ints([1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(joined(4 * 9), Ok(whole));
    assert_eq!(joined(4 * 9 - 1), Ok(exhausted));
}
