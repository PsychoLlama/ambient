//! Memoized runs: what a run observed decides whether it is made again.
//! See `spec/memoization.md` and `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::Cell;
use std::rc::Rc;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::core::{Expr as Core, Tree};
use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::literal::Literal;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::symbol::{ModuleId, Symbol};
use ambient::value::Value;
use ambient::workspace::Workspace;
use support::{value, workspace};

/// The world: one number native code can read. A negative one cannot be
/// read.
type World = Rc<Cell<i64>>;

/// Native code for `read`, which a run may or may not be able to repeat.
fn natives(world: &World, verifiable: bool) -> Natives {
    let world = Rc::clone(world);
    let read = move |_: &[Value]| match world.get() {
        number if number < 0 => Err("unset".to_owned()),
        number => Ok(Value::Int(number)),
    };
    if verifiable {
        Natives::new().verifiable("app", "main.read", read)
    } else {
        Natives::new().with("app", "main.read", read)
    }
}

/// `ask`, read natively under the target `build`, and `setting`, which
/// asks. `main` is whatever `setting` gives at compile time.
fn program(extra: impl IntoIterator<Item = Definition>) -> Workspace {
    let setting = b::function("setting")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::perform("ask", []));
    let main = b::function("main")
        .returns(ty::int())
        .body(b::comptime(b::call(b::name("setting"), [])));
    let module = b::module()
        .define(b::ability("ask").returns(ty::int()))
        .define(b::native_handler("read", "ask"))
        .define(b::target("build").bind("ask", "read"))
        .define(setting)
        .define(main);
    workspace([(
        "app",
        "main",
        extra.into_iter().fold(module, ModuleBuilder::define),
    )])
}

/// What one build did: what `main` was checked to, and the subjects each
/// stage did its work for.
struct Built {
    main: Result<Core, Error>,
    ran: Vec<String>,
    checked: Vec<String>,
}

/// The ability `ask`.
fn ask() -> Symbol {
    ModuleId::new("app", "main").ability("ask")
}

/// The program, with its package granted `ask`.
fn granted() -> Workspace {
    let mut workspace = program([]);
    workspace.grant("app", ask());
    workspace
}

/// Checks `main` over `store`, in `world`, with `ask` granted.
fn build(store: &MemoryStore, world: &World, verifiable: bool) -> Built {
    build_in(&granted(), store, world, verifiable)
}

/// Checks `main` of `workspace` over `store`, in `world`.
fn build_in(workspace: &Workspace, store: &MemoryStore, world: &World, verifiable: bool) -> Built {
    let engine = Engine::new(workspace, store)
        .with_build(value("main", "build"), natives(world, verifiable));
    let main = engine
        .check(&value("main", "main"))
        .map(|open| match &open.tree {
            Tree::Function(function) => function.body.clone(),
            _ => panic!("not a function"),
        });
    Built {
        main,
        ran: engine.ran(Stage::Run),
        checked: engine.ran(Stage::Check),
    }
}

fn int(value: i64) -> Core {
    Core::Literal(Literal::Int(value))
}

const SETTING: &str = "app:main.setting";
const MAIN: &str = "app:main.main";

#[test]
fn a_comptime_call_takes_abilities_from_the_build_target() {
    let built = build(&MemoryStore::new(), &World::new(Cell::new(5)), true);

    assert_eq!(built.main, Ok(int(5)));
    assert_eq!(built.ran, [SETTING]);
}

#[test]
fn a_run_is_reused_while_what_it_observed_still_holds() {
    let (store, world) = (MemoryStore::new(), World::new(Cell::new(5)));
    build(&store, &world, true);
    let again = build(&store, &world, true);

    assert_eq!(again.main, Ok(int(5)));
    assert!(again.ran.is_empty());
    assert!(again.checked.is_empty());
}

#[test]
fn a_run_is_made_again_when_what_it_observed_changed() {
    let (store, world) = (MemoryStore::new(), World::new(Cell::new(5)));
    build(&store, &world, true);
    world.set(6);
    let again = build(&store, &world, true);

    assert_eq!(again.main, Ok(int(6)));
    assert_eq!(again.ran, [SETTING]);
    assert!(again.checked.contains(&MAIN.to_owned()));
}

#[test]
fn a_run_that_observed_something_volatile_is_never_reused() {
    let (store, world) = (MemoryStore::new(), World::new(Cell::new(5)));
    build(&store, &world, false);
    let again = build(&store, &world, false);

    assert_eq!(again.ran, [SETTING]);
    // It gave what it gave before, so nothing built from it changes.
    assert!(again.checked.is_empty());
}

#[test]
fn a_failure_is_observed_like_any_result() {
    let (store, world) = (MemoryStore::new(), World::new(Cell::new(-1)));
    let failed = build(&store, &world, true);
    world.set(7);
    let fixed = build(&store, &world, true);

    assert!(matches!(failed.main, Err(Error::ComptimeFault { .. })));
    assert_eq!(fixed.main, Ok(int(7)));
}

// Grants

#[test]
fn a_comptime_call_takes_only_what_its_package_is_granted() {
    let world = World::new(Cell::new(5));
    let built = build_in(&program([]), &MemoryStore::new(), &world, true);
    let ungranted = Error::Ungranted {
        symbol: value("main", "main"),
        ability: ask(),
    };

    assert_eq!(built.main, Err(ungranted));
    assert!(built.ran.is_empty());
}

#[test]
fn a_grant_checks_the_call_again() {
    let (store, world) = (MemoryStore::new(), World::new(Cell::new(5)));
    build_in(&program([]), &store, &world, true);
    let granted = build(&store, &world, true);

    assert_eq!(granted.main, Ok(int(5)));
    assert!(granted.checked.contains(&MAIN.to_owned()));
}

#[test]
fn a_granted_call_still_needs_a_build_target() {
    let (workspace, store) = (granted(), MemoryStore::new());
    let result = Engine::new(&workspace, &store).check(&value("main", "main"));

    assert!(matches!(result, Err(Error::ComptimeAbilities(_))));
}

// Tests

/// Runs `test` under `quiet`, a target that answers `ask` with what
/// `answer` returns. Gives the result and whether the run was made.
fn test_run(store: &MemoryStore, answer: i64) -> (Result<Value, Error>, bool) {
    let constant = b::function("answer")
        .returns(ty::int())
        .body(b::int(answer));
    let test = b::function("test")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::add(b::perform("ask", []), b::int(1)));
    let quiet = b::target("quiet").bind("ask", "answer");
    let workspace = program([constant, test, quiet.into()]);
    let engine = Engine::new(&workspace, store);
    let (test, quiet) = (value("main", "test"), value("main", "quiet"));
    let result = engine.run_under(&test, &[], &quiet, &Natives::new());
    (result, !engine.ran(Stage::Run).is_empty())
}

#[test]
fn a_test_that_observes_nothing_reruns_only_when_its_program_changes() {
    let store = MemoryStore::new();

    assert_eq!(test_run(&store, 1), (Ok(Value::Int(2)), true));
    assert_eq!(test_run(&store, 1), (Ok(Value::Int(2)), false));
    // The handler is part of the program.
    assert_eq!(test_run(&store, 2), (Ok(Value::Int(3)), true));
}
