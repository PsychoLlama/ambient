//! Iterators of the standard library, run on every backend. See
//! `spec/iteration.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::library;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{run_in, workspace};

fn iterator_of(member: Type) -> Type {
    ty::apply("Iterator", [member])
}

/// `fn ones(): Iterator<int>`, an iterator that never ends, written as
/// a record.
fn ones() -> Definition {
    let step = b::tuple([b::int(1), b::call(b::name("ones"), [])]);
    b::function("ones")
        .returns(iterator_of(ty::int()))
        .body(b::record([("step", b::lambda([], step))]))
}

/// What `main`, with this body, gives beside `ones`, an ability
/// `ask(): int`, and the standard library.
fn evaluates(body: Expr) -> Value {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = b::module()
        .import("ambient.iterator")
        .define(ones())
        .define(b::ability("ask", "ask").returns(ty::int()))
        .define(main);
    let mut workspace = workspace([("app", "main", module)]);
    workspace.depend("app", "ambient", "ambient").unwrap();
    library::install(&mut workspace).unwrap();
    run_in(&workspace).unwrap()
}

fn ints<const N: usize>(members: [i64; N]) -> Value {
    Value::List(members.map(Value::Int).to_vec())
}

fn range(start: i64, end: i64) -> Expr {
    b::call(b::name("iterator.range"), [b::int(start), b::int(end)])
}

fn to_list(iterator: Expr) -> Expr {
    b::send(iterator, "to_list", [])
}

/// `(n: int) => body`
fn of_int(body: Expr) -> Expr {
    b::lambda([("n", Some(ty::int()))], body)
}

fn even() -> Expr {
    of_int(b::equal(b::remainder(b::name("n"), b::int(2)), b::int(0)))
}

fn sum(iterator: Expr) -> Expr {
    let params = [("total", Some(ty::int())), ("n", Some(ty::int()))];
    let add = b::lambda(params, b::add(b::name("total"), b::name("n")));
    b::send(iterator, "fold", [b::int(0), add])
}

#[test]
fn a_list_is_iterated_in_order() {
    let list = b::list([b::int(3), b::int(1), b::int(2)]);
    assert_eq!(
        evaluates(to_list(b::send(list, "iter", []))),
        ints([3, 1, 2])
    );
}

#[test]
fn a_member_that_is_null_is_not_the_end() {
    let list = b::list([b::int(1), b::null(), b::int(3)]);
    let expected = Value::List(vec![Value::Int(1), Value::Null, Value::Int(3)]);
    assert_eq!(evaluates(to_list(b::send(list, "iter", []))), expected);
}

#[test]
fn a_range_holds_every_integer_up_to_its_end() {
    assert_eq!(evaluates(to_list(range(2, 5))), ints([2, 3, 4]));
    assert_eq!(evaluates(to_list(range(5, 2))), ints([]));
}

#[test]
fn methods_transform_an_iterator_one_after_another() {
    let doubled = of_int(b::add(b::name("n"), b::name("n")));
    let evens = b::send(range(0, 100), "filter", [even()]);
    let firsts = b::send(b::send(evens, "map", [doubled]), "take", [b::int(3)]);
    assert_eq!(evaluates(to_list(firsts)), ints([0, 4, 8]));
}

#[test]
fn a_map_changes_the_type_of_the_items() {
    let texts = b::send(
        range(1, 3),
        "map",
        [of_int(b::send(b::name("n"), "to_text", []))],
    );
    let expected = Value::List(vec![Value::String("1".into()), Value::String("2".into())]);
    assert_eq!(evaluates(to_list(texts)), expected);
}

#[test]
fn a_fold_steps_an_iterator_to_its_end() {
    assert_eq!(evaluates(sum(range(1, 5))), Value::Int(10));
}

#[test]
fn a_record_with_a_step_is_an_iterator_and_may_be_endless() {
    let two = b::send(b::call(b::name("ones"), []), "take", [b::int(2)]);
    assert_eq!(evaluates(to_list(two)), ints([1, 1]));
}

#[test]
fn stepping_changes_nothing() {
    let twice = b::join(to_list(b::name("it")), to_list(b::name("it")));
    assert_eq!(
        evaluates(b::let_("it", range(0, 2), twice)),
        ints([0, 1, 0, 1])
    );
}

#[test]
fn a_long_iterator_is_folded_and_filtered_in_constant_stack() {
    assert_eq!(evaluates(sum(range(0, 100_000))), Value::Int(4_999_950_000));
    let none = of_int(b::boolean(false));
    let nothing = b::send(range(0, 100_000), "filter", [none]);
    assert_eq!(evaluates(to_list(nothing)), ints([]));
}

#[test]
fn what_steps_an_iterator_captures_the_abilities_it_performs() {
    let asked = of_int(b::add(b::name("n"), b::perform("ask", [])));
    let mapped = b::send(range(0, 2), "map", [asked]);
    // Made where `ask` answers 10, and stepped where it answers 20.
    let made = b::provide([("ask", b::lambda([], b::int(10)))], mapped);
    let stepped = b::provide([("ask", b::lambda([], b::int(20)))], to_list(b::name("it")));
    assert_eq!(evaluates(b::let_("it", made, stepped)), ints([10, 11]));
}
