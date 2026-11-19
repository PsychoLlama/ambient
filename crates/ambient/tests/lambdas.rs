//! Lambdas: functions written in place. See `docs/spec/definitions.md` and
//! `docs/spec/core.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::sync::Arc;

use ambient::builder::{self as b, ty};
use ambient::core::{Expr as Core, Open, Tree, Type as CoreType};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient::workspace::Workspace;
use support::{Recording, value, workspace};

fn program(definitions: impl IntoIterator<Item = Definition>) -> Workspace {
    let module = definitions
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    workspace([("app", "main", module)])
}

fn check(definitions: impl IntoIterator<Item = Definition>) -> Result<Arc<Open>, Error> {
    let store = MemoryStore::new();
    Engine::new(&program(definitions), &store).check(&value("main", "main"))
}

fn returning(ret: Type, body: Expr) -> Definition {
    b::function("main").returns(ret).body(body)
}

fn int_to_int() -> Type {
    ty::function([ty::int()], ty::int())
}

/// `fn(x) => x + 1`, with or without a type on `x`.
fn increment(typed: bool) -> Expr {
    b::lambda(
        [("x", typed.then(ty::int))],
        b::add(b::name("x"), b::int(1)),
    )
}

fn body(open: &Open) -> &Core {
    match &open.tree {
        Tree::Function(function) => &function.body,
        _ => panic!("not a function"),
    }
}

#[test]
fn a_lambda_is_a_function_value() {
    assert!(check([returning(int_to_int(), increment(true))]).is_ok());
    assert!(
        check([returning(
            ty::function([ty::string()], ty::int()),
            increment(true)
        )])
        .is_err()
    );
}

#[test]
fn a_parameter_takes_its_type_from_where_the_lambda_is_used() {
    assert!(check([returning(int_to_int(), increment(false))]).is_ok());
    assert!(matches!(
        check([returning(ty::unknown(), b::let_("f", increment(false), b::null()))]),
        Err(Error::UntypedParameter { name, .. }) if name == "x"
    ));
}

#[test]
fn a_function_value_is_called_like_any_function() {
    let main = returning(
        ty::int(),
        b::let_("f", increment(true), b::call(b::name("f"), [b::int(3)])),
    );

    assert!(check([main]).is_ok());
}

#[test]
fn a_lambda_captures_the_locals_it_uses() {
    let main = b::function("main")
        .param("unused", ty::int())
        .param("n", ty::int())
        .returns(int_to_int())
        .body(b::lambda([("x", None)], b::add(b::name("x"), b::name("n"))));
    let open = check([main]).unwrap();

    // The encloser hands over `n`, its second local.
    let Core::Closure { captures, .. } = body(&open) else {
        panic!("not a closure: {:?}", body(&open))
    };
    assert_eq!(captures, &[Core::Local(1)]);

    // Inside, the parameter is local 0 and `n` is capture 0.
    let [lambda] = open.lambdas.as_slice() else {
        panic!("expected one lambda")
    };
    let Tree::Function(function) = &lambda.tree else {
        panic!("not a function")
    };
    assert_eq!(function.captures, [CoreType::Int]);
    let Core::Primitive { args, .. } = &function.body else {
        panic!("not an operation")
    };
    assert_eq!(args, &[Core::Local(0), Core::Captured(0)]);
}

#[test]
fn a_capture_passes_through_every_lambda_between() {
    let thunk = |ret| ty::function([], ret);
    let main = b::function("main")
        .param("n", ty::int())
        .returns(thunk(thunk(ty::int())))
        .body(b::lambda([], b::lambda([], b::name("n"))));
    let open = check([main]).unwrap();

    let middle = &open.lambdas[0];
    let Core::Closure { captures, .. } = body(middle) else {
        panic!("not a closure")
    };
    assert_eq!(captures, &[Core::Captured(0)]);
    assert_eq!(body(&middle.lambdas[0]), &Core::Captured(0));
}

/// `fn apply<T, U>(v: T, f: Fn(T): U): U`, or with the parameters swapped.
fn apply(function_first: bool) -> Definition {
    let f = ("f", ty::function([ty::named("T")], ty::named("U")));
    let v = ("v", ty::named("T"));
    let (first, second) = if function_first { (f, v) } else { (v, f) };
    b::function("apply")
        .type_param("T")
        .type_param("U")
        .param(first.0, first.1)
        .param(second.0, second.1)
        .returns(ty::named("U"))
        .body(b::call(b::name("f"), [b::name("v")]))
}

#[test]
fn a_lambda_argument_is_typed_by_the_other_arguments() {
    let call = b::call(b::name("apply"), [b::int(1), increment(false)]);

    assert!(check([apply(false), returning(ty::int(), call.clone())]).is_ok());
    assert!(check([apply(false), returning(ty::string(), call)]).is_err());
}

#[test]
fn a_lambda_that_gives_back_what_it_takes_is_not_held_to_a_literal() {
    // `fn again<T>(v: T, f: Fn(T): T): T`: the literal says `T` is `1`,
    // and the lambda then gives an integer that is not.
    let f = ty::function([ty::named("T")], ty::named("T"));
    let again = b::function("again")
        .type_param("T")
        .param("v", ty::named("T"))
        .param("f", f)
        .returns(ty::named("T"))
        .body(b::call(b::name("f"), [b::name("v")]));
    let call = b::call(b::name("again"), [b::int(1), increment(false)]);

    assert!(check([again, returning(ty::int(), call)]).is_ok());
}

#[test]
fn arguments_are_checked_out_of_order_without_confusing_locals() {
    let lambda = b::lambda(
        [("x", None)],
        b::let_("a", b::name("x"), b::add(b::name("a"), b::int(1))),
    );
    let value = b::let_("b", b::int(2), b::name("b"));
    let call = b::call(b::name("apply"), [lambda, value]);
    // A binder after the call must still get the right number.
    let main = returning(ty::int(), b::let_("r", call, b::name("r")));

    assert!(check([apply(true), main]).is_ok());
}

#[test]
fn a_lambda_in_a_generic_function_shares_its_type_parameters() {
    let konst = b::function("main")
        .type_param("T")
        .param("v", ty::named("T"))
        .returns(ty::function([], ty::named("T")))
        .body(b::lambda([], b::name("v")));
    let open = check([konst]).unwrap();

    let Tree::Function(lambda) = &open.lambdas[0].tree else {
        panic!("not a function")
    };
    assert_eq!(lambda.type_params.len(), 1);
    assert_eq!(lambda.captures, [CoreType::Parameter(0)]);
}

#[test]
fn a_lambda_is_a_definition_of_its_own() {
    let make = |name: &str| {
        b::function(name)
            .returns(int_to_int())
            .body(increment(true))
    };
    let workspace = program([make("a"), make("b")]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let lambda = |name| value("main", name).lambda(&[0]);

    // Equal lambdas are one definition, wherever they are written.
    assert_eq!(
        engine.hash(&lambda("a")).unwrap(),
        engine.hash(&lambda("b")).unwrap()
    );
    assert_ne!(
        engine.hash(&lambda("a")).unwrap(),
        engine.hash(&value("main", "a")).unwrap()
    );
    assert!(engine.compile(&lambda("a"), &Recording::default()).is_ok());
}

#[test]
fn a_lambda_that_calls_its_encloser_forms_a_group_with_it() {
    let looping = |name: &str| {
        let again = b::call(b::call(b::name(name), []), []);
        b::function(name)
            .returns(ty::function([], ty::int()))
            .body(b::lambda([], again))
    };
    let hash = |name: &str| {
        let workspace = program([looping(name)]);
        let store = MemoryStore::new();
        Engine::new(&workspace, &store)
            .hash(&value("main", name))
            .unwrap()
    };

    assert_eq!(hash("spin"), hash("whirl"));
}

#[test]
fn a_lambda_gives_back_what_it_captured() {
    // The tuple is allocated, and outlives the closure that held it.
    let thunk = b::lambda([], b::name("pair"));
    let pair = b::tuple([b::int(1), b::int(2)]);
    let main = returning(ty::unknown(), b::let_("pair", pair, b::call(thunk, [])));

    assert_eq!(
        support::run([main], &[]),
        Ok(Value::Tuple(vec![Value::Int(1), Value::Int(2)]))
    );
}
