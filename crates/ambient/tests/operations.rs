//! Operations: what the language itself does with numbers, strings, and
//! lists. See `docs/spec/operations.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use ambient_runtime::{Fault, SMALL_MAX, SMALL_MIN};
use proptest::prelude::*;
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
fn an_integer_to_a_negative_power_truncates_as_division_does() {
    let power = |base, exponent| b::power(b::int(base), b::int(exponent));
    let powers = [
        power(2, 10),
        power(-3, 3),
        power(7, 0),
        power(2, -1),
        power(-1, -3),
    ];

    assert_eq!(each(powers), [1024, -27, 1, 0, -1].map(Value::Int));
}

#[test]
fn an_integer_power_faults_where_division_or_its_range_would() {
    let power = |base, exponent| outcome(b::power(b::int(base), b::int(exponent)));

    assert_eq!(power(0, -1), Err(Fault::DivideByZero));
    assert_eq!(power(2, 63), Err(Fault::Overflow));
    assert_eq!(power(-2, 63), Ok(Value::Int(i64::MIN)));
}

#[test]
fn a_float_power_is_what_c_says() {
    let power = |base, exponent| b::power(b::float(base), b::float(exponent));
    let powers = [power(2.0, 10.0), power(4.0, 0.5), power(2.0, -1.0)];
    let none = b::to_text(power(-8.0, 0.5));

    assert_eq!(each(powers), [1024.0, 2.0, 0.5].map(Value::Float));
    assert_eq!(outcome(none), Ok(text("NaN")));
}

#[test]
fn an_integer_and_a_float_do_not_divide() {
    let result = checks(ty::unknown(), b::divide(b::int(1), b::float(2.0)));

    assert!(matches!(result, Err(Error::Operands { .. })));
}

// Conversions

#[test]
fn an_integer_converts_to_the_float_nearest_it() {
    let floats = [b::to_float(b::int(3)), b::to_float(b::int(i64::MAX))];
    #[allow(clippy::cast_precision_loss)]
    let nearest = Value::Float(i64::MAX as f64);
    assert_eq!(each(floats), [Value::Float(3.0), nearest]);
}

#[test]
fn a_float_converts_to_an_integer_by_dropping_its_fraction() {
    let ints = [b::to_int(b::float(2.9)), b::to_int(b::float(-2.9))];
    assert_eq!(each(ints), [Value::Int(2), Value::Int(-2)]);
}

#[test]
fn a_float_that_is_no_integer_misses() {
    let none = [f64::NAN, f64::INFINITY, 1e19, -1e19].map(|float| b::to_int(b::float(float)));
    assert_eq!(each(none), vec![Value::Null; 4]);
}

#[test]
fn a_number_converts_to_text() {
    let texts = [
        b::to_text(b::int(-42)),
        b::to_text(b::float(1.0)),
        b::to_text(b::float(0.1)),
        b::to_text(b::float(1e100)),
    ];
    assert_eq!(each(texts), ["-42", "1.0", "0.1", "1e100"].map(text));
}

#[test]
fn text_converts_to_the_number_it_is() {
    let numbers = [
        b::to_int(b::string("-42")),
        b::to_float(b::string("0.1")),
        b::to_float(b::to_text(b::float(f64::MIN_POSITIVE))),
    ];
    let expected = [
        Value::Int(-42),
        Value::Float(0.1),
        Value::Float(f64::MIN_POSITIVE),
    ];
    assert_eq!(each(numbers), expected);
}

#[test]
fn text_that_is_not_wholly_a_number_misses() {
    let none = [
        b::to_int(b::string("42 ")),
        b::to_int(b::string("4.2")),
        b::to_int(b::string("9223372036854775808")),
        b::to_float(b::string("")),
    ];
    assert_eq!(each(none), vec![Value::Null; 4]);
}

#[test]
fn a_conversion_that_can_miss_says_so_in_its_type() {
    assert!(checks(ty::float(), b::to_float(b::int(1))).is_ok());
    assert!(checks(ty::int(), b::to_int(b::float(1.0))).is_err());
    assert!(checks(ty::string(), b::to_text(b::string("1"))).is_err());
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
fn no_list_operation_takes_a_tuple() {
    let pair = b::tuple([b::int(1), b::int(2)]);
    let result = checks(ty::int(), b::length(pair));

    assert!(matches!(result, Err(Error::Operands { .. })));
}

#[test]
fn a_tuple_is_not_a_list_of_its_members() {
    let pair = b::tuple([b::int(1), b::int(2)]);

    assert!(checks(ty::list(ty::int()), pair).is_err());
}

#[test]
fn only_strings_and_lists_have_a_length() {
    let result = checks(ty::int(), b::length(b::int(1)));

    assert!(matches!(result, Err(Error::Operands { .. })));
}

/// Integers near where an immediate one ends, and where any does.
fn edge() -> impl Strategy<Value = i64> {
    let ends = [0, SMALL_MIN, SMALL_MAX, i64::MIN + 8, i64::MAX - 8];
    (proptest::sample::select(ends.to_vec()), -8_i64..=8).prop_map(|(end, by)| end + by)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    // A backend answers some of these without the runtime, and must
    // give what the runtime would.
    #[test]
    fn arithmetic_is_the_same_at_every_size(left in edge(), right in edge()) {
        let (a, b) = (|| b::int(left), || b::int(right));
        let compared = each([b::less(a(), b()), b::equal(a(), b())]);
        prop_assert_eq!(compared, vec![Value::Bool(left < right), Value::Bool(left == right)]);

        let sum = outcome(b::add(a(), b())).ok();
        prop_assert_eq!(sum, left.checked_add(right).map(Value::Int));
        let difference = outcome(b::subtract(a(), b())).ok();
        prop_assert_eq!(difference, left.checked_sub(right).map(Value::Int));
    }
}
