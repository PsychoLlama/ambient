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

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

#[test]
fn strings() {
    assert_eq!(
        evaluates(ty::string(), b::string("héllo")),
        Value::String("héllo".to_owned())
    );
    let equal = |left, right| evaluates(ty::bool(), b::equal(b::string(left), b::string(right)));
    assert_eq!(equal("a", "a"), Value::Bool(true));
    assert_eq!(equal("a", "b"), Value::Bool(false));
}

#[test]
fn tuples_and_lists() {
    let members = || [b::int(1), b::string("a"), b::null()];
    let decoded = || vec![Value::Int(1), Value::String("a".to_owned()), Value::Null];

    assert_eq!(
        evaluates(ty::unknown(), b::tuple(members())),
        Value::Tuple(decoded())
    );
    assert_eq!(
        evaluates(ty::unknown(), b::list(members())),
        Value::List(decoded())
    );
    // A tuple and a list are different shapes, whatever they hold.
    let mixed = b::equal(
        b::tuple([b::int(1), b::int(2)]),
        b::list([b::int(1), b::int(2)]),
    );
    assert_eq!(evaluates(ty::bool(), mixed), Value::Bool(false));
}

#[test]
fn records() {
    let point = || b::record([("x", b::int(1)), ("y", b::string("a"))]);

    assert_eq!(
        evaluates(ty::unknown(), point()),
        record([("x", Value::Int(1)), ("y", Value::String("a".to_owned()))]),
    );
    assert_eq!(
        evaluates(ty::string(), b::field(point(), "y")),
        Value::String("a".to_owned())
    );
    // The order fields were written in is not part of the value.
    let swapped = b::record([("y", b::string("a")), ("x", b::int(1))]);
    assert_eq!(
        evaluates(ty::bool(), b::equal(point(), swapped)),
        Value::Bool(true)
    );
}

#[test]
fn nested_values_are_built_and_released() {
    let inner = b::list([
        b::int(i64::MAX),
        b::float(0.5),
        b::tuple([b::string("deep"), b::null()]),
    ]);
    let body = b::let_(
        "kept",
        b::record([("items", inner)]),
        b::field(b::name("kept"), "items"),
    );

    assert_eq!(
        evaluates(ty::unknown(), body),
        Value::List(vec![
            Value::Int(i64::MAX),
            Value::Float(0.5),
            Value::Tuple(vec![Value::String("deep".to_owned()), Value::Null]),
        ]),
    );
}

#[test]
fn a_match_takes_values_apart() {
    let shape = ty::union([
        ty::record([("kind", ty::text("square")), ("side", ty::int())]),
        ty::record([
            ("kind", ty::text("rectangle")),
            ("size", ty::tuple([ty::int(), ty::int()])),
        ]),
    ]);
    let area = b::function("area")
        .param("shape", shape)
        .returns(ty::int())
        .body(b::match_(
            b::name("shape"),
            [
                (
                    pat::record([("kind", pat::string("square")), ("side", pat::bind("s"))]),
                    b::multiply(b::name("s"), b::name("s")),
                ),
                (
                    pat::record([("size", pat::tuple([pat::bind("w"), pat::bind("h")]))]),
                    b::multiply(b::name("w"), b::name("h")),
                ),
            ],
        ));
    let call = |shape| {
        run(
            [
                area.clone(),
                main(ty::int(), b::call(b::name("area"), [shape])),
            ],
            &[],
        )
    };

    let square = b::record([("kind", b::string("square")), ("side", b::int(3))]);
    let rectangle = b::record([
        ("kind", b::string("rectangle")),
        ("size", b::tuple([b::int(2), b::int(5)])),
    ]);
    assert_eq!(call(square).unwrap(), Value::Int(9));
    assert_eq!(call(rectangle).unwrap(), Value::Int(10));
}

#[test]
fn a_tuple_pattern_does_not_match_a_list() {
    let first = b::function("first")
        .param("xs", ty::unknown())
        .returns(ty::unknown())
        .body(b::match_(
            b::name("xs"),
            [
                (pat::tuple([pat::bind("a"), pat::any()]), b::name("a")),
                (pat::any(), b::int(-1)),
            ],
        ));
    let call = |xs| {
        run(
            [
                first.clone(),
                main(ty::unknown(), b::call(b::name("first"), [xs])),
            ],
            &[],
        )
    };

    assert_eq!(
        call(b::tuple([b::int(7), b::int(8)])).unwrap(),
        Value::Int(7)
    );
    assert_eq!(
        call(b::list([b::int(7), b::int(8)])).unwrap(),
        Value::Int(-1)
    );
}

#[test]
fn recursive_data() {
    let node = ty::record([("head", ty::int()), ("tail", ty::named("Items"))]);
    let items = b::type_("Items", ty::union([ty::null(), node]));
    // `fn build(n: int): Items`, the numbers from `n` down to 1.
    let build = b::function("build")
        .param("n", ty::int())
        .returns(ty::named("Items"))
        .body(b::match_(
            b::name("n"),
            [
                (pat::int(0), b::null()),
                (
                    pat::any(),
                    b::record([
                        ("head", b::name("n")),
                        (
                            "tail",
                            b::call(b::name("build"), [b::subtract(b::name("n"), b::int(1))]),
                        ),
                    ]),
                ),
            ],
        ));
    let sum = b::function("sum")
        .param("items", ty::named("Items"))
        .returns(ty::int())
        .body(b::match_(
            b::name("items"),
            [
                (
                    pat::record([("head", pat::bind("h")), ("tail", pat::bind("t"))]),
                    b::add(b::name("h"), b::call(b::name("sum"), [b::name("t")])),
                ),
                (pat::any(), b::int(0)),
            ],
        ));
    let total = b::call(b::name("sum"), [b::call(b::name("build"), [b::int(2000)])]);

    assert_eq!(
        run([items, build, sum, main(ty::int(), total)], &[]).unwrap(),
        Value::Int(2_001_000)
    );
}

#[test]
fn function_values() {
    let double = b::function("double")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::multiply(b::name("n"), b::int(2)));
    let apply = b::function("apply")
        .param("f", ty::function([ty::int()], ty::int()))
        .param("v", ty::int())
        .returns(ty::int())
        .body(b::call(b::name("f"), [b::name("v")]));
    let named = b::call(b::name("apply"), [b::name("double"), b::int(21)]);
    let lambda = b::lambda([("x", None)], b::add(b::name("x"), b::int(1)));
    let written = b::call(b::name("apply"), [lambda, b::int(41)]);

    assert_eq!(
        run([double.clone(), apply.clone(), main(ty::int(), named)], &[]).unwrap(),
        Value::Int(42)
    );
    assert_eq!(
        run([apply, main(ty::int(), written)], &[]).unwrap(),
        Value::Int(42)
    );
    assert!(matches!(
        evaluates(ty::unknown(), b::lambda([], b::null())),
        Value::Function(_)
    ));
    let same = b::equal(b::name("double"), b::name("double"));
    assert_eq!(
        run([double, main(ty::bool(), same)], &[]).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn closures_capture_values() {
    let adder = b::function("adder")
        .param("n", ty::int())
        .returns(ty::function([ty::int()], ty::int()))
        .body(b::lambda([("x", None)], b::add(b::name("x"), b::name("n"))));
    let call = b::call(b::call(b::name("adder"), [b::int(2)]), [b::int(40)]);
    assert_eq!(
        run([adder, main(ty::int(), call)], &[]).unwrap(),
        Value::Int(42)
    );

    // A capture that passes through a lambda in between.
    let thunk = |ret| ty::function([], ret);
    let later = b::function("later")
        .param("v", ty::string())
        .returns(thunk(thunk(ty::string())))
        .body(b::lambda([], b::lambda([], b::name("v"))));
    let call = b::call(
        b::call(b::call(b::name("later"), [b::string("kept")]), []),
        [],
    );
    assert_eq!(
        run([later, main(ty::string(), call)], &[]).unwrap(),
        Value::String("kept".to_owned())
    );
}

#[test]
fn generic_functions_run_on_any_value() {
    let apply = b::function("apply")
        .type_param("T")
        .type_param("U")
        .param("v", ty::named("T"))
        .param("f", ty::function([ty::named("T")], ty::named("U")))
        .returns(ty::named("U"))
        .body(b::call(b::name("f"), [b::name("v")]));
    let pair = b::lambda([("x", None)], b::tuple([b::name("x"), b::name("x")]));
    let call = b::call(b::name("apply"), [b::string("a"), pair]);
    let twice = Value::Tuple(vec![Value::String("a".to_owned()); 2]);

    assert_eq!(run([apply, main(ty::unknown(), call)], &[]).unwrap(), twice);
}

#[test]
fn a_fault_inside_a_closure_unwinds_through_its_caller() {
    let overflow = b::lambda([("x", None)], b::add(b::name("x"), b::int(i64::MAX)));
    let body = b::let_(
        "held",
        b::list([b::string("a")]),
        b::let_(
            "f",
            overflow_typed(overflow),
            b::call(b::name("f"), [b::int(1)]),
        ),
    );

    assert_eq!(run([main(ty::int(), body)], &[]), Err(Fault::Overflow));
}

/// A lambda needs a parameter type when nothing expects one of it.
fn overflow_typed(lambda: Expr) -> Expr {
    match lambda {
        ambient::surface::Expr::Lambda {
            type_params,
            params,
            row,
            returns,
            body,
        } => ambient::surface::Expr::Lambda {
            type_params,
            returns,
            params: params
                .into_iter()
                .map(|(name, _)| (name, Some(ty::int())))
                .collect(),
            row,
            body,
        },
        other => other,
    }
}

/// `fn hide(v: unknown): unknown`, so that a cast has something to do.
fn hide() -> Definition {
    b::function("hide")
        .param("v", ty::unknown())
        .returns(ty::unknown())
        .body(b::name("v"))
}

/// Whether a value, seen as `unknown`, casts to `ty`.
fn casts(extra: &[Definition], value: Expr, ty: Type) -> bool {
    let hidden = b::call(b::name("hide"), [value]);
    let body = b::match_(
        b::cast(hidden, ty, b::string("no")),
        [
            (pat::string("no"), b::boolean(false)),
            (pat::any(), b::boolean(true)),
        ],
    );
    let mut definitions = extra.to_vec();
    definitions.extend([hide(), main(ty::bool(), body)]);
    run(definitions, &[]).unwrap() == Value::Bool(true)
}

#[test]
fn a_cast_tests_a_value_by_its_shape() {
    assert!(casts(&[], b::int(1), ty::int()));
    assert!(casts(&[], b::int(i64::MAX), ty::int()));
    assert!(!casts(&[], b::float(1.0), ty::int()));
    assert!(casts(&[], b::float(1.0), ty::float()));
    assert!(casts(&[], b::null(), ty::null()));
    assert!(casts(&[], b::boolean(false), ty::bool()));
    assert!(!casts(&[], b::int(0), ty::bool()));
    assert!(casts(&[], b::list([]), ty::unknown()));
}

#[test]
fn a_cast_to_a_literal_type_compares_the_value() {
    let either = || ty::union([ty::text("a"), ty::integer(1)]);

    assert!(casts(&[], b::string("a"), either()));
    assert!(casts(&[], b::int(1), either()));
    assert!(!casts(&[], b::string("b"), either()));
    assert!(!casts(&[], b::int(2), either()));
    assert!(casts(&[], b::boolean(true), ty::boolean(true)));
    assert!(!casts(&[], b::boolean(false), ty::boolean(true)));
}

#[test]
fn a_cast_looks_inside_structure() {
    let named = || ty::record([("name", ty::string())]);
    let person = b::record([("name", b::string("a")), ("age", b::int(1))]);

    // More fields than asked for is fine; fewer, or the wrong type, is not.
    assert!(casts(&[], person, named()));
    assert!(!casts(&[], b::record([("age", b::int(1))]), named()));
    assert!(!casts(&[], b::record([("name", b::int(1))]), named()));
    assert!(!casts(&[], b::string("a"), named()));

    let pair = || ty::tuple([ty::int(), ty::string()]);
    assert!(casts(&[], b::tuple([b::int(1), b::string("a")]), pair()));
    let longer = b::tuple([b::int(1), b::string("a"), b::null()]);
    assert!(!casts(&[], longer, pair()));
    assert!(!casts(&[], b::list([b::int(1), b::string("a")]), pair()));

    // A tuple is not a list, whatever it holds.
    assert!(casts(
        &[],
        b::list([b::int(1), b::int(2)]),
        ty::list(ty::int())
    ));
    assert!(!casts(
        &[],
        b::tuple([b::int(1), b::int(2)]),
        ty::list(ty::int())
    ));
    assert!(!casts(
        &[],
        b::list([b::int(1), b::string("a")]),
        ty::list(ty::int())
    ));
}

fn chain(head: Expr, tail: Expr) -> Expr {
    b::record([("head", head), ("tail", tail)])
}

#[test]
fn a_cast_to_a_recursive_type_follows_the_value() {
    let node = ty::record([("head", ty::int()), ("tail", ty::named("Items"))]);
    let items = [b::type_("Items", ty::union([ty::null(), node]))];
    let good = chain(b::int(1), chain(b::int(2), b::null()));
    let bad = chain(b::int(1), chain(b::string("two"), b::null()));

    assert!(casts(&items, good, ty::named("Items")));
    assert!(casts(&items, b::null(), ty::named("Items")));
    assert!(!casts(&items, bad, ty::named("Items")));
    assert!(!casts(&items, b::int(1), ty::named("Items")));
}

#[test]
fn a_cast_to_a_generic_type_applies_its_arguments() {
    let node = ty::record([
        ("head", ty::named("T")),
        ("tail", ty::apply("Items", [ty::named("T")])),
    ]);
    let items = [b::generic_type(
        "Items",
        ["T"],
        ty::union([ty::null(), node]),
    )];
    let numbers = || chain(b::int(1), chain(b::int(2), b::null()));
    let of = |member| ty::apply("Items", [member]);

    assert!(casts(&items, numbers(), of(ty::int())));
    assert!(!casts(&items, numbers(), of(ty::string())));
    assert!(casts(
        &items,
        numbers(),
        of(ty::union([ty::int(), ty::string()]))
    ));
    // An argument that is itself an application.
    let nested = chain(numbers(), b::null());
    assert!(casts(&items, nested, of(of(ty::int()))));
}

#[test]
fn a_type_test_in_a_match_picks_the_arm() {
    let describe = b::function("describe")
        .param("v", ty::union([ty::int(), ty::string(), ty::null()]))
        .returns(ty::int())
        .body(b::match_(
            b::name("v"),
            [
                (
                    pat::bind_as("n", pat::is(ty::int())),
                    b::add(b::name("n"), b::int(1)),
                ),
                (pat::is(ty::null()), b::int(-1)),
                (pat::any(), b::int(0)),
            ],
        ));
    let call = |value| {
        let main = main(ty::int(), b::call(b::name("describe"), [value]));
        run([describe.clone(), main], &[]).unwrap()
    };

    assert_eq!(call(b::int(41)), Value::Int(42));
    assert_eq!(call(b::null()), Value::Int(-1));
    assert_eq!(call(b::string("a")), Value::Int(0));
}
