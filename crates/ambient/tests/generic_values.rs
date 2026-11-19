//! Values of generic function types, run on every backend. See
//! "Generics" in `docs/spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{run, value, workspace};

fn t() -> Type {
    ty::named("T")
}

/// `<U>(U) -> U`
fn identity() -> Type {
    ty::generic(
        [("U", None)],
        ty::function([ty::named("U")], ty::named("U")),
    )
}

/// `fn id<T>(v: T): T`
fn id() -> Definition {
    b::function("id")
        .type_param("T")
        .param("v", t())
        .returns(t())
        .body(b::name("v"))
}

/// `fn is_a<T>(v: unknown): bool`, which needs its type argument as it
/// runs.
fn is_a() -> Definition {
    let arms = [
        (pat::is(t()), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    b::function("is_a")
        .type_param("T")
        .param("v", ty::unknown())
        .returns(ty::bool())
        .body(b::match_(b::name("v"), arms))
}

/// `(f(1), f("a"))`
fn at_two_types(f: &str) -> Expr {
    let call = |arg| b::call(b::name(f), [arg]);
    b::tuple([call(b::int(1)), call(b::string("a"))])
}

fn both() -> Value {
    Value::Tuple(vec![Value::Int(1), Value::String("a".into())])
}

/// What `body` evaluates to, beside `id`, `is_a`, and `extra`.
fn evaluates(body: Expr, extra: impl IntoIterator<Item = Definition>) -> Value {
    let main = b::function("main").returns(ty::unknown()).body(body);
    run([id(), is_a(), main].into_iter().chain(extra), &[]).unwrap()
}

#[test]
fn a_generic_function_is_a_value_used_at_more_than_one_type() {
    let body = b::let_("f", b::name("id"), at_two_types("f"));
    assert_eq!(evaluates(body, []), both());
}

#[test]
fn a_generic_function_is_passed_to_a_function_that_asks_for_one() {
    let twice = b::function("twice")
        .param("f", identity())
        .returns(ty::unknown())
        .body(at_two_types("f"));
    let body = b::call(b::name("twice"), [b::name("id")]);
    assert_eq!(evaluates(body, [twice]), both());
}

#[test]
fn a_generic_value_is_instantiated_where_a_function_is_expected() {
    let apply = b::function("apply")
        .param("f", ty::function([ty::int()], ty::int()))
        .returns(ty::int())
        .body(b::call(b::name("f"), [b::int(3)]));
    let body = b::let_(
        "g",
        b::name("id"),
        b::call(b::name("apply"), [b::name("g")]),
    );
    assert_eq!(evaluates(body, [apply]), Value::Int(3));
}

#[test]
fn a_generic_value_that_needs_its_type_arguments_is_handed_them() {
    let at = |member: Type, value: Expr| b::call(b::instantiate("f", [member]), [value]);
    let tests = b::tuple([
        at(ty::int(), b::int(1)),
        at(ty::int(), b::string("a")),
        at(ty::string(), b::string("a")),
    ]);
    let expected = [true, false, true].map(Value::Bool);
    let body = b::let_("f", b::name("is_a"), tests);
    assert_eq!(evaluates(body, []), Value::Tuple(expected.to_vec()));
}

#[test]
fn a_record_holds_a_generic_function_in_a_field() {
    let record = b::record([("same", b::name("id"))]);
    let sent = |arg| b::send(b::name("record"), "same", [arg]);
    let body = b::tuple([sent(b::int(1)), sent(b::string("a"))]);
    assert_eq!(evaluates(b::let_("record", record, body), []), both());
}

/// Whether `value`, its type forgotten, is of type `ty`.
fn is(ty: Type, value: Expr) -> Expr {
    let unknown = b::cast(value, ty::unknown(), b::null());
    let arms = [
        (pat::is(ty), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    b::match_(unknown, arms)
}

#[test]
fn a_generic_function_comes_back_out_of_unknown_as_what_it_is() {
    let plain = ty::function([ty::int()], ty::int());
    let tests = b::tuple([
        is(identity(), b::name("id")),
        is(plain.clone(), b::name("id")),
        is(identity(), b::instantiate("is_a", [ty::int()])),
    ]);
    let expected = [true, false, false].map(Value::Bool);
    assert_eq!(evaluates(tests, []), Value::Tuple(expected.to_vec()));
}

#[test]
fn generic_functions_are_compared_as_values() {
    let same = b::equal(b::name("id"), b::name("id"));
    let other = b::equal(
        b::instantiate("is_a", [ty::int()]),
        b::instantiate("is_a", [ty::string()]),
    );
    let expected = [true, false].map(Value::Bool);
    assert_eq!(
        evaluates(b::tuple([same, other]), []),
        Value::Tuple(expected.to_vec())
    );
}

/// `<T>(v: T) => body`
fn generic_lambda(body: Expr) -> Expr {
    b::generic([("T", None)], b::lambda([("v", Some(t()))], body))
}

#[test]
fn a_lambda_with_type_parameters_is_used_at_more_than_one_type() {
    let body = b::let_("f", generic_lambda(b::name("v")), at_two_types("f"));
    assert_eq!(evaluates(body, []), both());
}

#[test]
fn a_generic_lambda_captures_what_is_in_scope() {
    let paired = generic_lambda(b::tuple([b::name("kept"), b::name("v")]));
    let called = b::let_("f", paired, b::call(b::name("f"), [b::string("a")]));
    assert_eq!(evaluates(b::let_("kept", b::int(1), called), []), both());
}

#[test]
fn a_generic_function_gives_a_generic_function() {
    let second = ty::generic(
        [("B", None)],
        ty::function(
            [ty::named("B")],
            ty::tuple([ty::named("A"), ty::named("B")]),
        ),
    );
    let inner = b::lambda(
        [("b", Some(ty::named("B")))],
        b::tuple(["a", "b"].map(b::name)),
    );
    let pair = b::function("pair")
        .type_param("A")
        .param("a", ty::named("A"))
        .returns(second)
        .body(b::generic([("B", None)], inner));
    let made = b::call(b::name("pair"), [b::int(1)]);
    assert_eq!(evaluates(b::call(made, [b::string("a")]), [pair]), both());
}

#[test]
fn a_generic_lambda_is_handed_its_type_arguments_after_its_enclosers() {
    let is = |name: &str| {
        let arms = [
            (pat::is(ty::named(name)), b::boolean(true)),
            (pat::any(), b::boolean(false)),
        ];
        b::match_(b::name("v"), arms)
    };
    let inner = b::lambda([("v", Some(ty::unknown()))], b::tuple([is("A"), is("B")]));
    let gives = ty::function([ty::unknown()], ty::tuple([ty::bool(), ty::bool()]));
    let tests = b::function("tests")
        .type_param("A")
        .returns(ty::generic([("B", None)], gives))
        .body(b::generic([("B", None)], inner));
    let made = b::call(b::instantiate("tests", [ty::int()]), []);
    let called = b::call(b::instantiate("g", [ty::string()]), [b::string("a")]);
    let expected = [false, true].map(Value::Bool);
    assert_eq!(
        evaluates(b::let_("g", made, called), [tests]),
        Value::Tuple(expected.to_vec())
    );
}

#[test]
fn a_generic_lambda_takes_parameter_types_from_what_is_expected() {
    let twice = b::function("twice")
        .param("f", identity())
        .returns(ty::unknown())
        .body(at_two_types("f"));
    let untyped = b::generic([("T", None)], b::lambda([("v", None)], b::name("v")));
    let body = b::call(b::name("twice"), [untyped]);
    assert_eq!(evaluates(body, [twice]), both());
}

#[test]
fn a_type_parameter_of_a_lambda_is_known_by_its_bound() {
    let bounded = |arg: Expr| {
        let add = b::lambda([("v", Some(t()))], b::add(b::name("v"), b::int(1)));
        let lambda = b::generic([("T", Some(ty::int()))], add);
        let main = b::function("main").returns(ty::unknown());
        let body = b::let_("f", lambda, b::call(b::name("f"), [arg]));
        let workspace = workspace([("app", "main", b::module().define(main.body(body)))]);
        let store = MemoryStore::new();
        Engine::new(&workspace, &store)
            .check(&value("main", "main"))
            .is_ok()
    };
    assert!(bounded(b::int(1)));
    assert!(!bounded(b::string("a")));
}

fn has_x() -> Type {
    ty::record([("x", ty::int())])
}

/// `{x, with}`, where `with` is `<U>(self, other: U) => (self.x, other)`.
/// The receiver has the type given, or the record's own if none is.
fn pairing(x: i64, receiver: Option<Type>) -> Expr {
    let pair = b::tuple([b::field(b::name("self"), "x"), b::name("other")]);
    let params = [("self", receiver), ("other", Some(ty::named("U")))];
    let with = b::generic([("U", None)], b::lambda(params, pair));
    b::record([("x", b::int(x)), ("with", b::method(with))])
}

/// `(record.with(1), record.with("a"))`, of the record bound to `name`.
fn with_both(name: &str) -> Expr {
    let with = |arg| b::send(b::name(name), "with", [arg]);
    b::tuple([with(b::int(1)), with(b::string("a"))])
}

fn paired(x: i64) -> Value {
    let with = |other| Value::Tuple(vec![Value::Int(x), other]);
    Value::Tuple(vec![with(Value::Int(1)), with(Value::String("a".into()))])
}

#[test]
fn a_method_with_type_parameters_is_called_at_more_than_one_type() {
    let body = b::let_("record", pairing(7, Some(has_x())), with_both("record"));
    assert_eq!(evaluates(body, []), paired(7));
}

#[test]
fn a_generic_method_may_leave_its_receivers_type_out() {
    let body = b::let_("record", pairing(7, None), with_both("record"));
    assert_eq!(evaluates(body, []), paired(7));
}

#[test]
fn reading_a_generic_method_gives_a_generic_function_bound_to_its_record() {
    let read = b::field(pairing(7, Some(has_x())), "with");
    let body = b::let_("with", read, at_two_types("with"));
    assert_eq!(evaluates(body, []), paired(7));
}

#[test]
fn a_generic_method_receives_the_record_it_is_merged_into() {
    let merged = b::merge(pairing(7, Some(has_x())), b::record([("x", b::int(9))]));
    let body = b::let_("record", merged, with_both("record"));
    assert_eq!(evaluates(body, []), paired(9));
}

#[test]
fn a_record_type_asks_for_a_generic_method() {
    let gives = ty::tuple([ty::int(), ty::named("U")]);
    let with = ty::generic([("U", None)], ty::function([ty::named("U")], gives));
    let asked = ty::record([("with", ty::method(has_x(), with))]);
    let both = b::function("both")
        .param("record", asked)
        .returns(ty::unknown())
        .body(with_both("record"));
    let body = b::call(b::name("both"), [pairing(7, Some(has_x()))]);
    assert_eq!(evaluates(body, [both]), paired(7));
}

#[test]
fn a_generic_method_that_needs_its_type_arguments_is_handed_them() {
    let arms = [
        (pat::is(ty::named("U")), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    let params = [
        ("self", Some(ty::record([]))),
        ("like", Some(ty::named("U"))),
        ("v", Some(ty::unknown())),
    ];
    let alike = b::generic(
        [("U", None)],
        b::lambda(params, b::match_(b::name("v"), arms)),
    );
    let record = b::record([("alike", b::method(alike))]);
    // A sum is an `int`, where a literal would be only itself.
    let int = || b::add(b::int(0), b::int(1));
    let sent = |v| b::send(b::name("record"), "alike", [int(), v]);
    let read = b::call(b::field(b::name("record"), "alike"), [int(), b::int(2)]);
    let tests = b::tuple([sent(b::int(2)), sent(b::string("a")), read]);
    let expected = [true, false, true].map(Value::Bool);
    assert_eq!(
        evaluates(b::let_("record", record, tests), []),
        Value::Tuple(expected.to_vec())
    );
}

/// `type Tree<T> = {value: T, map: <U>((T) -> U) -> Tree<U>}`, where
/// `map` is a method, and `fn leaf<T>(value: T): Tree<T>`.
fn tree() -> [Definition; 2] {
    let u = || ty::named("U");
    let of = |member: Type| ty::apply("Tree", [member]);
    let takes = ty::function([t()], u());
    let map = ty::generic([("U", None)], ty::function([takes.clone()], of(u())));
    let fields = [("value", t()), ("map", ty::method(of(t()), map))];
    let mapped = b::call(b::name("f"), [b::field(b::name("self"), "value")]);
    let params = [
        ("self", Some(ty::record([("value", t())]))),
        ("f", Some(takes)),
    ];
    let method = b::lambda(params, b::call(b::name("leaf"), [mapped]));
    let record = b::record([
        ("value", b::name("value")),
        ("map", b::method(b::generic([("U", None)], method))),
    ]);
    let leaf = b::function("leaf")
        .type_param("T")
        .param("value", t())
        .returns(of(t()));
    [
        b::generic_type("Tree", ["T"], ty::record(fields)),
        leaf.body(record),
    ]
}

#[test]
fn a_method_gives_its_own_type_at_another_type_argument() {
    let text = b::lambda([("n", None)], b::to_text(b::name("n")));
    let mapped = b::send(b::call(b::name("leaf"), [b::int(7)]), "map", [text]);
    assert_eq!(
        evaluates(b::field(mapped, "value"), tree()),
        Value::String("7".into())
    );
}

#[test]
fn a_cell_of_a_type_that_applies_itself_to_other_types_is_tested() {
    let pairs = |member: Type| ty::apply("Pairs", [member]);
    let deeper = pairs(ty::tuple([t(), t()]));
    let node = ty::record([("head", t()), ("next", deeper)]);
    let definition = b::generic_type("Pairs", ["T"], ty::union([ty::null(), node]));
    let cell = || b::cell(pairs(ty::int()), b::null());
    let tests = b::tuple([
        is(ty::cell(pairs(ty::int())), cell()),
        is(ty::cell(pairs(ty::string())), cell()),
    ]);
    let expected = [true, false].map(Value::Bool);
    assert_eq!(
        evaluates(tests, [definition]),
        Value::Tuple(expected.to_vec())
    );
}

#[test]
fn a_generic_lambda_comes_back_out_of_unknown_as_what_it_is() {
    let plain = ty::function([ty::int()], ty::int());
    let tests = b::tuple([
        is(identity(), generic_lambda(b::name("v"))),
        is(plain, generic_lambda(b::name("v"))),
    ]);
    let expected = [true, false].map(Value::Bool);
    assert_eq!(evaluates(tests, []), Value::Tuple(expected.to_vec()));
}

/// `fn keeper<T>(kept: T): () -> T`, whose lambda is of a type that
/// depends on `T`, and holds `T` or not as `holds` says.
fn keeper(holds: bool) -> Definition {
    let tested = b::cast(b::name("kept"), t(), b::name("kept"));
    let body = if holds { tested } else { b::name("kept") };
    b::function("keeper")
        .type_param("T")
        .param("kept", t())
        .returns(ty::function([], t()))
        .body(b::lambda([], body))
}

/// Whether what `keeper` gives for an integer is a function that gives
/// an integer, and whether it is one that gives a string.
fn kept_is(holds: bool) -> Value {
    let made = || b::call(b::instantiate("keeper", [ty::int()]), [b::int(1)]);
    let gives = |ty| ty::function([], ty);
    let tests = [
        is(gives(ty::int()), made()),
        is(gives(ty::string()), made()),
    ];
    evaluates(b::tuple(tests), [keeper(holds)])
}

#[test]
fn a_function_is_of_its_type_at_the_type_arguments_it_holds() {
    let expected = [true, false].map(Value::Bool);
    assert_eq!(kept_is(true), Value::Tuple(expected.to_vec()));
}

#[test]
fn a_function_that_holds_no_type_arguments_says_no_type_that_depends_on_them() {
    let expected = [false, false].map(Value::Bool);
    assert_eq!(kept_is(false), Value::Tuple(expected.to_vec()));
}

#[test]
fn a_generic_function_at_type_arguments_it_holds_is_a_function_of_that_type() {
    // `is_a<T>(v: unknown): bool` tests against `T`, so it holds it.
    let at_int = || b::instantiate("is_a", [ty::int()]);
    let plain = ty::function([ty::unknown()], ty::bool());
    let other = ty::function([ty::int()], ty::bool());
    let tests = b::tuple([is(plain, at_int()), is(other, at_int())]);
    let expected = [true, false].map(Value::Bool);
    assert_eq!(evaluates(tests, []), Value::Tuple(expected.to_vec()));
}
