//! Budgets, run on every backend. See `spec/budgets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::{FAULT_PACKAGE, Fault};
use support::tasks::{note, then};
use support::{Recording, run, value, workspace};

/// `fn count(n: int): int`, which calls itself `n` times and gives `n`.
fn count() -> Definition {
    let less = b::call(b::name("count"), [b::subtract(b::name("n"), b::int(1))]);
    b::function("count")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [
                (pat::int(0), b::int(0)),
                (pat::any(), b::add(less, b::int(1))),
            ],
        ))
}

/// `fn build(n: int): unknown`, which makes a tuple nested `n` deep.
fn build() -> Definition {
    let less = b::call(b::name("build"), [b::subtract(b::name("n"), b::int(1))]);
    b::function("build")
        .param("n", ty::int())
        .returns(ty::unknown())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), b::null()), (pat::any(), b::tuple([less]))],
        ))
}

fn counts(n: i64) -> Expr {
    b::call(b::name("count"), [b::int(n)])
}

fn budgeted(steps: i64, allocation: i64, body: Expr) -> Expr {
    b::budget(b::int(steps), b::int(allocation), body)
}

/// Runs `body` with a log.
fn outcome(body: Expr) -> Result<Value, Fault> {
    let body = b::let_("log", b::cell(ty::int(), b::int(0)), body);
    let main = b::function("main").returns(ty::unknown()).body(body);
    run([count(), build(), note(), main], &[])
}

fn evaluates(body: Expr) -> Value {
    outcome(body).unwrap()
}

/// The limit a budget gives when it runs out.
fn limit(name: &str) -> Value {
    Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: name.to_owned(),
    }
}

fn notes(entry: i64) -> Expr {
    b::call(b::name("note"), [b::name("log"), b::int(entry)])
}

/// What `body` gives, and the log once it is over.
fn logged(body: Expr) -> (Value, Value) {
    let pair = b::tuple([b::name("result"), b::read(b::name("log"))]);
    match evaluates(b::let_("result", body, pair)) {
        Value::Tuple(ref members) => (members[0].clone(), members[1].clone()),
        other => panic!("expected a pair, got {other:?}"),
    }
}

#[test]
fn a_budget_that_is_enough_gives_what_its_body_does() {
    assert_eq!(evaluates(budgeted(1000, 1000, counts(10))), Value::Int(10));
}

#[test]
fn a_budget_that_runs_out_of_steps_gives_that_limit() {
    assert_eq!(evaluates(budgeted(5, 1000, counts(10))), limit("steps"));
}

#[test]
fn a_budget_that_runs_out_of_allocation_gives_that_limit() {
    let builds = b::call(b::name("build"), [b::int(50)]);

    assert_eq!(evaluates(budgeted(1000, 20, builds)), limit("allocation"));
}

#[test]
fn a_call_costs_one_step() {
    // `count(3)` is four calls.
    assert_eq!(evaluates(budgeted(4, 1000, counts(3))), Value::Int(3));
    assert_eq!(evaluates(budgeted(3, 1000, counts(3))), limit("steps"));
}

#[test]
fn the_program_goes_on_after_a_budget_runs_out() {
    let body = b::tuple([budgeted(2, 1000, counts(10)), counts(10)]);

    assert_eq!(
        evaluates(body),
        Value::Tuple(vec![limit("steps"), Value::Int(10)])
    );
}

#[test]
fn a_charge_counts_against_every_budget_around_it() {
    // The inner budget would be enough; the outer one is not.
    let inner = budgeted(1000, 1000, counts(10));

    assert_eq!(evaluates(budgeted(5, 1000, inner)), limit("steps"));
}

#[test]
fn an_inner_budget_that_runs_out_leaves_the_outer_one_running() {
    let inner = budgeted(2, 1000, counts(10));
    let body = budgeted(1000, 1000, b::tuple([inner, counts(3)]));

    assert_eq!(
        evaluates(body),
        Value::Tuple(vec![limit("steps"), Value::Int(3)])
    );
}

#[test]
fn tasks_started_inside_a_budget_draw_from_it_together() {
    // Either task alone fits; the two together do not.
    let tasks = b::all([counts(5), counts(5)]);

    assert_eq!(evaluates(budgeted(9, 1000, tasks)), limit("steps"));
}

#[test]
fn a_budget_that_runs_out_ends_its_body_before_what_it_cannot_afford() {
    let body = then(notes(1), then(counts(10), notes(2)));

    assert_eq!(logged(budgeted(6, 1000, body)).1, Value::Int(1));
}

#[test]
fn a_fault_inside_a_budget_is_still_a_fault() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));

    assert_eq!(outcome(budgeted(10, 1000, overflow)), Err(Fault::Overflow));
}

#[test]
fn a_boundary_does_not_contain_a_budget_running_out() {
    let body = budgeted(2, 1000, b::boundary(counts(10)));

    assert_eq!(evaluates(body), limit("steps"));
}

// Handlers

/// `ability work(): int`
fn work() -> Definition {
    b::ability("work").returns(ty::int()).into()
}

/// Runs `body` beside `work`.
fn working(body: Expr) -> Value {
    let main = b::function("main").returns(ty::unknown()).body(body);
    run([count(), work(), main], &[]).unwrap()
}

/// `body` with `work` handled by a hundred calls.
fn provided(body: Expr) -> Expr {
    b::provide([("work", b::lambda([], counts(100)))], body)
}

#[test]
fn a_handler_bound_outside_a_budget_costs_it_one_step() {
    let performs = || b::perform("work", []);

    assert_eq!(
        working(provided(budgeted(1, 1000, performs()))),
        Value::Int(100)
    );
    assert_eq!(
        working(provided(budgeted(0, 1000, performs()))),
        limit("steps")
    );
}

#[test]
fn a_handler_bound_inside_a_budget_draws_from_it() {
    let body = budgeted(50, 1000, provided(b::perform("work", [])));

    assert_eq!(working(body), limit("steps"));
}

#[test]
fn a_handler_bound_between_two_budgets_draws_from_the_outer_one() {
    let inner = || budgeted(5, 1000, b::perform("work", []));

    assert_eq!(
        working(budgeted(1000, 1000, provided(inner()))),
        Value::Int(100)
    );
    assert_eq!(
        working(budgeted(50, 1000, provided(inner()))),
        limit("steps")
    );
}

#[test]
fn a_function_value_carries_the_budget_its_handlers_were_bound_inside() {
    // The lambda captures `work` outside the budget and is called inside.
    let made = provided(b::lambda([], b::perform("work", [])));
    let body = b::let_("f", made, budgeted(2, 1000, b::call(b::name("f"), [])));

    assert_eq!(working(body), Value::Int(100));
}

// Cost

/// How many charges the plain form of `name` makes: its code as what is
/// outside every budget calls it, in a program whose `main` is `body`.
fn charges(name: &str, body: Expr) -> usize {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = [count(), build(), main]
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = ambient::store::MemoryStore::new();
    let engine = ambient::engine::Engine::new(&workspace, &store);
    let open = engine.check(&value("main", name)).unwrap();
    let plain = ambient::lower::lower(&open, &ambient::lower::Form::default()).unwrap();

    format!("{:?}", plain.body).matches("Charge").count()
}

#[test]
fn code_that_runs_outside_a_budget_makes_no_charge() {
    // `count` runs inside this budget too, but not as its plain form.
    assert_eq!(charges("count", budgeted(10, 10, counts(1))), 0);
}

#[test]
fn only_the_body_of_a_budget_is_metered_in_the_function_that_enters_it() {
    let alone = charges("main", budgeted(10, 10, counts(1)));
    let after_work = charges("main", then(counts(5), budgeted(10, 10, counts(1))));

    assert!(alone > 0);
    assert_eq!(after_work, alone);
}

#[test]
fn a_function_made_outside_a_budget_is_metered_when_called_inside_it() {
    let made = b::lambda([], counts(5));
    let body = b::let_("work", made, budgeted(3, 100, b::call(b::name("work"), [])));

    assert_eq!(evaluates(body), limit("steps"));
}

/// Whether `name` has a metered form in a program whose `main` is
/// `body`.
fn has_metered_form(name: &str, body: Expr) -> bool {
    use ambient::engine::{Body, Engine};

    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = [count(), build(), main]
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = ambient::store::MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let parts = engine
        .link(&value("main", "main"), &Recording::default())
        .unwrap();
    let hash = engine.hash(&value("main", name)).unwrap();
    let part = parts.parts.iter().find(|part| part.hash == hash).unwrap();
    matches!(
        part.body,
        Body::Function {
            metered: Some(_),
            ..
        }
    )
}

#[test]
fn only_what_can_run_inside_a_budget_has_a_metered_form() {
    let body = then(
        b::call(b::name("build"), [b::int(1)]),
        budgeted(10, 10, counts(1)),
    );

    assert!(has_metered_form("count", body.clone()));
    assert!(!has_metered_form("build", body));
    assert!(!has_metered_form("count", counts(1)));
}

#[test]
fn a_function_used_as_a_value_has_a_metered_form_wherever_it_is_made() {
    // `build` is never named inside the budget, but it gets there.
    let body = b::let_(
        "work",
        b::name("build"),
        budgeted(10, 10, b::call(b::name("work"), [b::int(1)])),
    );

    assert!(has_metered_form("build", body));
}
