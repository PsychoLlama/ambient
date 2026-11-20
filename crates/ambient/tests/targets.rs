//! Targets and externs: a program run under a world. See
//! `docs/spec/targets.md` and `docs/spec/externs.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::{Native, Natives, Program};
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::{run_with, value, workspace};

/// `ability <name>(): int`, tagged with its name.
fn ability(name: &str) -> Definition {
    b::ability(name).returns(ty::int()).into()
}

/// `fn <name>(): int`, which returns `value`.
fn constant(name: &str, value: i64) -> Definition {
    b::function(name).returns(ty::int()).body(b::int(value))
}

/// `fn main(): [ask] <ret>`
fn main(ret: Type, body: Expr) -> Definition {
    b::function("main")
        .performs(["ask"])
        .returns(ret)
        .body(body)
}

fn asks() -> Expr {
    b::perform("ask", [])
}

/// Native code that takes nothing and returns `value`.
fn returns(tag: &'static str, value: i64) -> impl Fn() -> Natives {
    move || Natives::new().with("app", tag, move |_| Ok(Value::Int(value)))
}

/// Runs `main` under `target`, beside `ask`, `base`, and `definitions`.
fn runs(
    definitions: impl IntoIterator<Item = Definition>,
    natives: impl Fn() -> Natives,
) -> Result<Value, Fault> {
    let abilities = [ability("ask"), ability("base")];
    run_with(
        Some("target"),
        abilities.into_iter().chain(definitions),
        natives,
    )
}

// Root bindings

#[test]
fn a_program_runs_as_its_entry_under_its_target() {
    let target = b::target("target").bind("ask", "answer");
    let definitions = [
        constant("answer", 42),
        target.into(),
        main(ty::int(), asks()),
    ];

    assert_eq!(runs(definitions, Natives::new), Ok(Value::Int(42)));
}

#[test]
fn a_root_handler_takes_its_row_from_the_targets_other_bindings() {
    // `fn doubled(): [base] int`, bound to `ask`.
    let doubled = b::function("doubled")
        .performs(["base"])
        .returns(ty::int())
        .body(b::multiply(b::perform("base", []), b::int(2)));
    let target = b::target("target")
        .bind("ask", "doubled")
        .bind("base", "ten");
    let definitions = [
        doubled,
        constant("ten", 10),
        target.into(),
        main(ty::int(), asks()),
    ];

    assert_eq!(runs(definitions, Natives::new), Ok(Value::Int(20)));
}

#[test]
fn an_abort_handled_at_the_root_ends_the_program_with_the_handlers_result() {
    let fail = b::ability("fail")
        .param("reason", ty::string())
        .returns(ty::never());
    let recover = b::function("recover")
        .param("reason", ty::string())
        .returns(ty::string())
        .body(b::name("reason"));
    let main = b::function("main")
        .performs(["fail"])
        .returns(ty::int())
        .body(b::perform("fail", [b::string("stop")]));
    let target = b::target("target").bind("fail", "recover");

    let outcome = runs([fail.into(), recover, target.into(), main], Natives::new);
    assert_eq!(outcome, Ok(Value::String("stop".to_owned())));
}

#[test]
fn a_target_extends_another_and_replaces_what_it_binds_again() {
    let base = b::target("plain").bind("ask", "one").bind("base", "ten");
    let target = b::target("target").extends("plain").bind("ask", "two");
    let body = b::add(asks(), b::perform("base", []));
    let main = b::function("main")
        .performs(["ask", "base"])
        .returns(ty::int())
        .body(body);
    let definitions = [
        constant("one", 1),
        constant("two", 2),
        constant("ten", 10),
        base.into(),
        target.into(),
        main,
    ];

    assert_eq!(runs(definitions, Natives::new), Ok(Value::Int(12)));
}

// Native code

#[test]
fn a_native_handler_implements_an_ability() {
    let read = b::native_handler("read", "ask");
    let target = b::target("target").bind("ask", "read");
    let definitions = [read.into(), target.into(), main(ty::int(), asks())];

    assert_eq!(
        runs(definitions, returns("main.read", 7)),
        Ok(Value::Int(7))
    );
}

#[test]
fn native_code_is_given_the_arguments_and_gives_back_any_value() {
    // `ability pair(a: int, b: string): (text: string, number: int)`
    let pair = b::ability("pair")
        .param("a", ty::int())
        .param("b", ty::string())
        .returns(ty::tuple([ty::string(), ty::int()]));
    let swap = b::native_handler("swap", "pair");
    let main = b::function("main")
        .performs(["pair"])
        .returns(ty::tuple([ty::string(), ty::int()]))
        .body(b::perform("pair", [b::int(1), b::string("one")]));
    let target = b::target("target").bind("pair", "swap");
    let natives = || {
        Natives::new().with("app", "main.swap", |args| {
            Ok(Value::Tuple(vec![args[1].clone(), args[0].clone()]))
        })
    };

    let expected = Value::Tuple(vec![Value::String("one".to_owned()), Value::Int(1)]);
    assert_eq!(
        runs([pair.into(), swap.into(), target.into(), main], natives),
        Ok(expected)
    );
}

#[test]
fn an_extern_function_is_called_like_any_other() {
    let double = b::extern_function("double")
        .param("n", ty::int())
        .returns(ty::int());
    let main = b::function("main")
        .returns(ty::int())
        .body(b::call(b::name("double"), [b::int(21)]));
    let natives = || {
        Natives::new().with("app", "main.double", |args| match args {
            [Value::Int(n)] => Ok(Value::Int(n * 2)),
            _ => Err("not an integer".to_owned()),
        })
    };

    assert_eq!(
        run_with(None, [double.into(), main], natives),
        Ok(Value::Int(42))
    );
}

#[test]
fn native_code_that_fails_is_a_fault() {
    let read = b::native_handler("read", "ask");
    let target = b::target("target").bind("ask", "read");
    let definitions = [read.into(), target.into(), main(ty::int(), asks())];
    let natives = || Natives::new().with("app", "main.read", |_| Err("no input".to_owned()));

    assert_eq!(runs(definitions, natives), Err(Fault::Native));
}

#[test]
fn a_binding_the_entry_does_not_reach_is_not_linked() {
    // Nothing supplies `unused`, and nothing needs it.
    let read = b::native_handler("read", "ask");
    let unused = b::native_handler("unused", "base");
    let target = b::target("target")
        .bind("ask", "read")
        .bind("base", "unused");
    let definitions = [
        read.into(),
        unused.into(),
        target.into(),
        main(ty::int(), asks()),
    ];

    assert_eq!(
        runs(definitions, returns("main.read", 7)),
        Ok(Value::Int(7))
    );
}

// What is refused

fn module(definitions: impl IntoIterator<Item = Definition>) -> ModuleBuilder {
    [ability("ask"), ability("base")]
        .into_iter()
        .chain(definitions)
        .fold(b::module(), ModuleBuilder::define)
}

/// Checks `name`, defined beside `ask`, `base`, and `definitions`.
fn check(name: &str, definitions: impl IntoIterator<Item = Definition>) -> Result<(), Error> {
    let workspace = workspace([("app", "main", module(definitions))]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", name))
        .map(|_| ())
}

#[test]
fn an_entry_takes_only_what_its_target_offers() {
    let target = b::target("target").bind("base", "ten");
    let definitions = [constant("ten", 10), target.into(), main(ty::int(), asks())];
    let workspace = workspace([("app", "main", module(definitions))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let root = engine
        .program(&value("main", "main"), &value("main", "target"))
        .unwrap();

    assert!(matches!(engine.check(&root), Err(Error::Unoffered { .. })));
}

#[test]
fn a_program_is_not_loaded_without_the_native_code_it_reaches() {
    let read = b::native_handler("read", "ask");
    let target = b::target("target").bind("ask", "read");
    let definitions = [read.into(), target.into(), main(ty::int(), asks())];
    let workspace = workspace([("app", "main", module(definitions))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let root = engine
        .program(&value("main", "main"), &value("main", "target"))
        .unwrap();
    let parts = engine.link(&root, &Native::new().unwrap()).unwrap();

    assert!(Program::load(&parts, &Natives::new()).is_err());
}

#[test]
fn a_native_handler_and_a_target_are_not_values() {
    let read: Definition = b::native_handler("read", "ask").into();
    let target: Definition = b::target("target").bind("ask", "read").into();
    let calls = |name: &str| {
        b::function("main")
            .returns(ty::int())
            .body(b::call(b::name(name), []))
    };
    let with = |main| check("main", [read.clone(), target.clone(), main]);

    assert!(matches!(with(calls("read")), Err(Error::NotValue { .. })));
    assert!(matches!(with(calls("target")), Err(Error::NotValue { .. })));
}

#[test]
fn a_root_handler_must_fit_its_ability() {
    let text = b::function("text")
        .returns(ty::string())
        .body(b::string("no"));
    let target = b::target("target").bind("ask", "text");

    let result = check("target", [text, target.into()]);
    assert!(matches!(result, Err(Error::Mismatch { .. })));
}

/// A native handler of an ability that does not return takes the trace
/// of the abort, as `system`'s handler of `throw` does.
#[test]
fn a_target_is_checked_with_a_root_handler_that_takes_the_trace() {
    let fail = b::ability("fail")
        .param("reason", ty::string())
        .returns(ty::never());
    let stop = b::native_handler("stop", "fail");
    let target = b::target("target").bind("fail", "stop");

    assert!(check("target", [fail.into(), stop.into(), target.into()]).is_ok());
}

#[test]
fn root_handlers_cannot_need_each_other() {
    let through = |name: &str, needs: &str| {
        b::function(name)
            .performs([needs])
            .returns(ty::int())
            .body(b::perform(needs, []))
    };
    let target = b::target("target").bind("ask", "a").bind("base", "b");
    let definitions = [through("a", "base"), through("b", "ask"), target.into()];

    assert!(matches!(
        check("target", definitions),
        Err(Error::TargetCycle(_))
    ));
}

#[test]
fn a_conflict_between_extended_targets_is_settled_by_a_binding() {
    let targets = |settled: bool| {
        let both = b::target("target").extends("first").extends("second");
        let both = if settled {
            both.bind("ask", "two")
        } else {
            both
        };
        [
            constant("one", 1),
            constant("two", 2),
            b::target("first").bind("ask", "one").into(),
            b::target("second").bind("ask", "two").into(),
            both.into(),
        ]
    };

    assert!(matches!(
        check("target", targets(false)),
        Err(Error::TargetConflict { .. })
    ));
    assert!(check("target", targets(true)).is_ok());
}
