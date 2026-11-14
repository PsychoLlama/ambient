//! The core checker: what a receiver refuses. See `spec/exchange.md`.
//!
//! That it accepts what checking makes is tested by every program
//! `support::run` runs. Here a checked tree is tampered with, as a
//! sender that is not this compiler might, and must be refused.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::core::{Expr, Function, Pattern, Primitive, Slot, Tree, Type};
use ambient::engine::Engine;
use ambient::literal::Literal;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::verify::{Invalid, verify};
use support::{value, workspace};

/// `ability ask(): int`, `fn helper(n: int): int`, and
/// `fn main(x: int): [ask] int`, which uses both.
fn program() -> [Definition; 3] {
    let helper = b::function("helper")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::name("n"));
    let body = b::add(
        b::call(b::name("helper"), [b::name("x")]),
        b::perform("ask", []),
    );
    let main = b::function("main")
        .param("x", ty::int())
        .performs(["ask"])
        .returns(ty::int())
        .body(body);
    [b::ability("ask").returns(ty::int()).into(), helper, main]
}

/// Verifies `main` of [`program`] after `tamper` has been at it.
fn tampered(tamper: impl FnOnce(&mut Function)) -> Result<(), Invalid> {
    let module = program()
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let mut open = (*engine.check(&value("main", "main")).unwrap()).clone();
    if let Tree::Function(function) = &mut open.tree {
        tamper(function);
    }
    verify(&open, false, &|symbol| engine.check(symbol).ok())
}

/// Verifies `main` with its body replaced.
fn with_body(body: Expr) -> Result<(), Invalid> {
    tampered(|function| function.body = body)
}

fn int(value: i64) -> Expr {
    Expr::Literal(Literal::Int(value))
}

fn bind(value: Expr, body: Expr) -> Expr {
    Expr::Let {
        value: Box::new(value),
        body: Box::new(body),
    }
}

#[test]
fn what_checking_made_is_accepted() {
    assert_eq!(tampered(|_| {}), Ok(()));
}

#[test]
fn a_result_of_the_wrong_type_is_refused() {
    assert!(matches!(
        with_body(Expr::Null),
        Err(Invalid::Mismatch { .. })
    ));
}

#[test]
fn a_field_the_value_may_lack_is_refused() {
    let read = Expr::Field {
        target: Box::new(Expr::Local(0)),
        key: ambient::core::Key::Field("a".to_owned()),
    };
    assert!(matches!(with_body(read), Err(Invalid::Shape(_))));
}

#[test]
fn a_local_that_was_never_bound_is_refused() {
    assert_eq!(with_body(Expr::Local(7)), Err(Invalid::Local));
}

#[test]
fn a_local_read_outside_its_scope_is_refused() {
    // Local 1 is bound for the first `let`'s value only.
    let inner = bind(int(1), Expr::Local(1));
    assert_eq!(with_body(bind(inner.clone(), Expr::Local(2))), Ok(()));
    assert_eq!(with_body(bind(inner, Expr::Local(1))), Err(Invalid::Local));
}

#[test]
fn a_slot_outside_the_table_is_refused() {
    let nothing = Expr::Reference(Slot(9), Vec::new());
    assert_eq!(with_body(nothing), Err(Invalid::Slot));
}

#[test]
fn a_call_with_too_few_arguments_is_refused() {
    // Slot 0 is `ask`, which the header names first; slot 1 is `helper`.
    let call = |args| Expr::Call {
        callee: Box::new(Expr::Reference(Slot(1), Vec::new())),
        args,
    };
    assert_eq!(with_body(call(vec![int(1)])), Ok(()));
    assert_eq!(with_body(call(Vec::new())), Err(Invalid::Arity));
}

#[test]
fn an_operation_on_the_wrong_operands_is_refused() {
    let add = |args| Expr::Primitive {
        operation: Primitive::IntAdd,
        args,
    };
    let text = Expr::Literal(Literal::String("one".to_owned()));
    assert!(matches!(
        with_body(add(vec![text, int(1)])),
        Err(Invalid::Shape(_))
    ));
    assert!(matches!(
        with_body(add(vec![int(1)])),
        Err(Invalid::Shape(_))
    ));
}

#[test]
fn a_perform_nothing_binds_is_refused() {
    let unbound = tampered(|function| function.row.clear());
    assert!(matches!(unbound, Err(Invalid::Unbound(_))));
}

#[test]
fn a_match_that_lets_a_value_through_is_refused() {
    let one = Pattern::Literal(Literal::Int(1));
    let matching = |arms| Expr::Match {
        subject: Box::new(Expr::Local(0)),
        arms,
    };
    let rest = (Pattern::Wildcard, int(0));
    assert_eq!(
        with_body(matching(vec![(one.clone(), int(1)), rest])),
        Ok(())
    );
    assert_eq!(
        with_body(matching(vec![(one, int(1))])),
        Err(Invalid::Match)
    );
}

#[test]
fn a_type_not_in_its_one_form_is_refused() {
    let returning = |members| tampered(|function| function.ret = Type::Union(members));
    assert_eq!(returning(vec![Type::Null, Type::Int]), Ok(()));
    assert_eq!(returning(vec![Type::Int, Type::Null]), Err(Invalid::Type));
}

#[test]
fn a_test_for_a_function_type_that_mentions_a_type_parameter_is_refused() {
    let function = |ret| Type::Function {
        params: Vec::new(),
        ret: Box::new(ret),
        row: Vec::new(),
    };
    let cast = |ty| {
        tampered(|main| {
            main.type_params = vec![Type::Unknown];
            main.ret = Type::Unknown;
            main.body = Expr::Cast {
                value: Box::new(int(1)),
                ty,
                otherwise: Box::new(int(2)),
            };
        })
    };

    assert_eq!(cast(function(Type::Null)), Ok(()));
    let open = cast(function(Type::Parameter(0)));
    assert!(matches!(open, Err(Invalid::Shape(_))));
}

#[test]
fn a_task_spawned_outside_every_group_is_refused() {
    let spawn = Expr::Spawn(Box::new(Expr::Null));
    assert!(matches!(with_body(spawn), Err(Invalid::Shape(_))));
}

#[test]
fn a_task_that_takes_what_is_bound_inside_its_group_is_refused() {
    // `provide ask { group { spawn(ask()) } }`: the task takes `ask`
    // from outside its group.
    let task = b::group(b::spawn(b::perform("ask", [])));
    let body = b::provide([("ask", b::lambda([], b::int(7)))], task);
    let module = b::module()
        .define(b::ability("ask").returns(ty::int()))
        .define(b::function("main").returns(ty::unknown()).body(body));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let mut open = (*engine.check(&value("main", "main")).unwrap()).clone();
    let verified =
        |open: &ambient::core::Open| verify(open, false, &|symbol| engine.check(symbol).ok());
    assert_eq!(verified(&open), Ok(()));

    // The same with the group outside the provide.
    let Tree::Function(function) = &mut open.tree else {
        panic!("not a function")
    };
    // The bundle is bound first; the provide is under that.
    let mut at = &mut function.body;
    while let Expr::Let { body, .. } = at {
        at = body;
    }
    let Expr::Provide { body, .. } = at else {
        panic!("not a provide")
    };
    let Expr::Group(inner) = std::mem::replace(&mut **body, Expr::Null) else {
        panic!("not a group")
    };
    **body = *inner;
    let provide = std::mem::replace(at, Expr::Null);
    *at = Expr::Group(Box::new(provide));

    assert!(matches!(verified(&open), Err(Invalid::Shape(_))));
}

#[test]
fn only_a_root_names_a_native_handler() {
    let module = b::module()
        .define(b::ability("ask").returns(ty::int()))
        .define(b::native_handler("answer", "ask"))
        .define(b::target("world").bind("ask", "answer"))
        .define(
            b::function("main")
                .performs(["ask"])
                .returns(ty::int())
                .body(b::perform("ask", [])),
        );
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let root = engine
        .program(&value("main", "main"), &value("main", "world"))
        .unwrap();
    let open = engine.check(&root).unwrap();
    let verified = |root| verify(&open, root, &|symbol| engine.check(symbol).ok());
    assert_eq!(verified(true), Ok(()));
    assert!(matches!(verified(false), Err(Invalid::Shape(_))));
}
