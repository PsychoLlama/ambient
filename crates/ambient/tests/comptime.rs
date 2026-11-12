//! Comptime calls: functions run during compilation. See
//! `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ModuleBuilder, pat, ty};
use ambient::core::{Expr as Core, Tree};
use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::literal::Literal;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient::workspace::Workspace;
use support::{run, value, workspace};

fn program(definitions: impl IntoIterator<Item = Definition>) -> Workspace {
    let module = definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    workspace([("app", "main", module)])
}

/// `fn factorial(n: int): int`, counting down to `base`.
fn factorial(base: i64) -> Definition {
    let recurse = b::call(b::name("factorial"), [b::subtract(b::name("n"), b::int(1))]);
    b::function("factorial")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::less(b::name("n"), b::int(base + 1)),
            [
                (pat::boolean(true), b::int(1)),
                (pat::boolean(false), b::multiply(b::name("n"), recurse)),
            ],
        ))
}

fn main(ret: Type, body: Expr) -> Definition {
    b::function("main").returns(ret).body(body)
}

fn at_compile_time(n: i64) -> Expr {
    b::comptime(b::call(b::name("factorial"), [b::int(n)]))
}

/// The body `main` was checked to.
fn body(definitions: impl IntoIterator<Item = Definition>) -> Result<Core, Error> {
    let store = MemoryStore::new();
    let open = Engine::new(&program(definitions), &store).check(&value("main", "main"))?;
    match &open.tree {
        Tree::Function(function) => Ok(function.body.clone()),
        Tree::Type { .. } => panic!("not a function"),
    }
}

#[test]
fn the_result_replaces_the_call() {
    let spliced = body([factorial(0), main(ty::int(), at_compile_time(10))]).unwrap();

    assert_eq!(spliced, Core::Literal(Literal::Int(3_628_800)));
}

#[test]
fn the_result_has_a_singleton_type() {
    let exact = main(ty::integer(120), at_compile_time(5));
    let wrong = main(ty::integer(121), at_compile_time(5));

    assert!(body([factorial(0), exact]).is_ok());
    assert!(matches!(
        body([factorial(0), wrong]),
        Err(Error::Mismatch { .. })
    ));
}

#[test]
fn the_hash_covers_the_result_not_the_call() {
    let hash = |definitions: Vec<Definition>| {
        let store = MemoryStore::new();
        Engine::new(&program(definitions), &store)
            .hash(&value("main", "main"))
            .unwrap()
    };
    let called = hash(vec![factorial(0), main(ty::int(), at_compile_time(5))]);
    let written = hash(vec![main(ty::int(), b::int(120))]);
    // Another way to compute the same thing.
    let other = hash(vec![factorial(1), main(ty::int(), at_compile_time(5))]);

    assert_eq!(called, written);
    assert_eq!(called, other);
}

#[test]
fn the_program_runs_with_the_result_in_place() {
    let main = main(ty::int(), b::add(at_compile_time(5), b::int(1)));

    assert_eq!(run([factorial(0), main], &[]).unwrap(), Value::Int(121));
}

#[test]
fn an_argument_must_be_known_from_its_type() {
    let main = b::function("main")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::comptime(b::call(b::name("factorial"), [b::name("n")])));

    assert!(matches!(
        body([factorial(0), main]),
        Err(Error::NotComptime { found, .. }) if found == "int"
    ));
}

#[test]
fn arguments_and_results_may_be_structured() {
    let swap = b::function("swap")
        .param("pair", ty::record([("a", ty::int()), ("b", ty::string())]))
        .returns(ty::unknown())
        .body(b::tuple([
            b::field(b::name("pair"), "b"),
            b::list([b::field(b::name("pair"), "a")]),
        ]));
    let argument = b::record([("a", b::int(1)), ("b", b::string("x"))]);
    let call = b::comptime(b::call(b::name("swap"), [argument]));
    let spliced = body([swap, main(ty::unknown(), call)]).unwrap();

    assert_eq!(
        spliced,
        Core::Tuple(vec![
            Core::Literal(Literal::String("x".to_owned())),
            Core::List(vec![Core::Literal(Literal::Int(1))]),
        ]),
    );
}

#[test]
fn a_function_is_not_a_result() {
    let make = b::function("make")
        .returns(ty::function([], ty::null()))
        .body(b::lambda([], b::null()));
    let call = b::comptime(b::call(b::name("make"), []));

    assert!(matches!(
        body([make, main(ty::unknown(), call)]),
        Err(Error::NotSpliceable { .. })
    ));
}

#[test]
fn a_fault_during_compilation_is_a_compile_error() {
    assert!(matches!(
        body([factorial(0), main(ty::int(), at_compile_time(30))]),
        Err(Error::ComptimeFault { fault, .. }) if fault == "Overflow"
    ));
}

#[test]
fn a_definition_cannot_depend_on_its_own_result() {
    let main = main(ty::int(), b::comptime(b::call(b::name("main"), [])));

    assert!(matches!(body([main]), Err(Error::ComptimeCycle(_))));
}

#[test]
fn a_call_runs_once_and_reruns_when_its_function_changes() {
    let store = MemoryStore::new();
    let main = || main(ty::int(), at_compile_time(5));
    let check = |definitions: Vec<Definition>| {
        let workspace = program(definitions);
        let engine = Engine::new(&workspace, &store);
        engine.check(&value("main", "main")).unwrap();
        (engine.ran(Stage::Run).len(), engine.ran(Stage::Check))
    };

    let (ran, _) = check(vec![factorial(0), main()]);
    assert_eq!(ran, 1);

    // Nothing changed: neither the call nor the check runs again.
    let (ran, checked) = check(vec![factorial(0), main()]);
    assert_eq!((ran, checked.len()), (0, 0));

    // The function changed, so the call runs again; its result is the
    // same, but `main` was keyed by the function, so it is checked again.
    let (ran, checked) = check(vec![factorial(1), main()]);
    assert_eq!(ran, 1);
    assert!(checked.contains(&"app:main.main".to_owned()));
}
