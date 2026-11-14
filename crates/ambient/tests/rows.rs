//! What the checker says about abilities, and what of them is content.
//! See `spec/abilities.md` and `spec/handlers.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Native;
use ambient::builder::{self as b, BuildError, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use support::{value, workspace};

/// `ability <name>(): int`, tagged with its name.
fn ability(name: &str) -> Definition {
    b::ability(name).returns(ty::int()).into()
}

/// `fn twice(): [ask] int`
fn twice() -> Definition {
    b::function("twice")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::add(b::perform("ask", []), b::perform("ask", [])))
}

fn module(definitions: impl IntoIterator<Item = Definition>) -> ModuleBuilder {
    definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define)
}

/// Checks `main(): ret` beside `ask`, `base`, `twice`, and `extra`.
fn check(ret: Type, body: Expr, extra: impl IntoIterator<Item = Definition>) -> Result<(), Error> {
    let main = b::function("main").returns(ret).body(body);
    let definitions = [ability("ask"), ability("base"), twice(), main];
    let workspace = workspace([("app", "main", module(definitions.into_iter().chain(extra)))]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .map(|_| ())
}

fn unbound(result: Result<(), Error>) -> String {
    match result {
        Err(Error::Unbound { ability, .. }) => ability.name.clone(),
        other => panic!("expected an unbound ability, got {other:?}"),
    }
}

fn answers(value: Expr) -> Expr {
    b::lambda([], value)
}

fn asking(body: Expr) -> Expr {
    b::provide([("ask", answers(b::int(1)))], body)
}

// Scope

#[test]
fn an_ability_must_be_bound_to_be_performed() {
    assert_eq!(unbound(check(ty::int(), b::perform("ask", []), [])), "ask");
    assert!(check(ty::int(), asking(b::perform("ask", [])), []).is_ok());
}

#[test]
fn a_call_needs_the_callees_row_bound() {
    let call = || b::call(b::name("twice"), []);

    assert_eq!(unbound(check(ty::int(), call(), [])), "ask");
    assert!(check(ty::int(), asking(call()), []).is_ok());
}

#[test]
fn an_only_provide_hides_what_is_bound_outside_it() {
    let hidden = b::provide_only([("base", answers(b::int(1)))], b::perform("ask", []));

    assert_eq!(unbound(check(ty::int(), asking(hidden), [])), "ask");
}

#[test]
fn a_lambda_cannot_capture_what_nothing_binds() {
    let body = b::call(b::lambda([], b::perform("ask", [])), []);

    assert_eq!(unbound(check(ty::int(), body, [])), "ask");
}

#[test]
fn binding_in_place_needs_the_ability_bound_there() {
    // `twice` takes `ask`; a `Fn(): int` takes nothing.
    let pure = ty::function([], ty::int());

    assert_eq!(unbound(check(pure, b::name("twice"), [])), "ask");
}

// Types

#[test]
fn a_function_that_takes_less_stands_in_for_one_that_takes_more() {
    let wide = ty::function_row([], ["ask", "base"], ty::int());

    assert!(check(wide, b::name("twice"), []).is_ok());
}

#[test]
fn a_function_inside_a_value_is_not_bound_in_place() {
    // Only a function value itself is bound; one inside a list would have
    // to be rebuilt.
    let pure = ty::list(ty::function([], ty::int()));
    let result = check(pure, asking(b::list([b::name("twice")])), []);

    assert!(matches!(result, Err(Error::Mismatch { .. })));
}

#[test]
fn a_handler_must_fit_the_signature_of_its_ability() {
    let wrong = b::provide([("ask", answers(b::string("no")))], b::int(1));
    let result = check(ty::int(), wrong, []);

    assert!(matches!(result, Err(Error::Mismatch { .. })));
}

#[test]
fn a_perform_passes_what_the_ability_takes() {
    let result = check(ty::int(), asking(b::perform("ask", [b::int(1)])), []);

    assert!(matches!(
        result,
        Err(Error::Arity {
            expected: 0,
            found: 1,
            ..
        })
    ));
}

#[test]
fn a_provide_has_the_type_of_its_body_or_of_a_handler_that_aborts() {
    let fail = b::ability("fail").returns(ty::never());
    let body = || {
        b::provide(
            [("fail", answers(b::string("failed")))],
            b::let_("unused", b::perform("fail", []), b::int(1)),
        )
    };

    assert!(
        check(
            ty::union([ty::int(), ty::string()]),
            body(),
            [fail.clone().into()]
        )
        .is_ok()
    );
    assert!(check(ty::int(), body(), [fail.into()]).is_err());
}

// Bundles

#[test]
fn a_provide_takes_a_record_of_handlers_and_nothing_else() {
    let not_a_record = b::with(b::int(1), b::null());
    let has_a_field = b::with(b::record([("ask", answers(b::int(1)))]), b::null());

    assert!(matches!(
        check(ty::null(), not_a_record, []),
        Err(Error::NotBundle { .. })
    ));
    assert!(matches!(
        check(ty::null(), has_a_field, []),
        Err(Error::NotBundle { .. })
    ));
}

#[test]
fn a_bundle_holds_one_handler_for_an_ability_however_it_is_named() {
    // `ask` and `abilities.ask` are one ability.
    let abilities = module([ability("ask").public()]);
    let twice = b::bundle([
        ("ask", answers(b::int(1))),
        ("abilities.ask", answers(b::int(2))),
    ]);
    let main = b::module()
        .import("pkg.abilities")
        .import("pkg.abilities.ask")
        .define(b::function("main").body(b::with(twice, b::null())));
    let workspace = workspace([("app", "abilities", abilities), ("app", "main", main)]);
    let store = MemoryStore::new();
    let result = Engine::new(&workspace, &store).check(&value("main", "main"));

    assert!(
        matches!(result, Err(Error::DuplicateHandler { .. })),
        "{result:?}"
    );
}

#[test]
fn only_records_merge() {
    let result = check(ty::unknown(), b::merge(b::int(1), b::record([])), []);

    assert!(matches!(result, Err(Error::Operands { .. })));
}

#[test]
fn a_merge_has_the_fields_of_both_with_the_right_sides_types() {
    let left = b::record([("a", b::int(1)), ("b", b::int(2))]);
    let right = b::record([("b", b::string("two"))]);
    let ret = ty::record([("a", ty::int()), ("b", ty::string())]);

    assert!(check(ret, b::merge(left, right), []).is_ok());
}

// What cannot be provided

#[test]
fn a_comptime_call_takes_nothing_from_where_it_is_written() {
    let body = asking(b::comptime(b::call(b::name("twice"), [])));
    let result = check(ty::int(), body, []);

    // A handler around the call is not a grant.
    assert!(matches!(result, Err(Error::Ungranted { .. })));
}

#[test]
fn an_entry_cannot_take_abilities() {
    let workspace = workspace([("app", "main", module([ability("ask"), twice()]))]);
    let store = MemoryStore::new();
    let result =
        Engine::new(&workspace, &store).link(&value("main", "twice"), &Native::new().unwrap());

    assert!(matches!(result, Err(Error::EntryAbilities(_))));
}

// Builders

#[test]
fn a_provide_binds_an_ability_once() {
    let twice = b::provide(
        [("ask", answers(b::int(1))), ("ask", answers(b::int(2)))],
        b::null(),
    );
    let module = b::module().define(b::function("main").body(twice));

    assert!(matches!(
        module.build(),
        Err(BuildError::DuplicateHandler(_))
    ));
}

// Content

/// The hash of `user` in a module of `definitions`.
fn hash_of(user: &str, definitions: impl IntoIterator<Item = Definition>) -> Hash {
    let workspace = workspace([("app", "main", module(definitions))]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .hash(&value("main", user))
        .unwrap()
}

/// `fn user(f: Fn(): [<row>] int): int`
fn user(row: [&str; 2]) -> Definition {
    b::function("user")
        .param("f", ty::function_row([], row, ty::int()))
        .returns(ty::int())
        .body(b::int(0))
}

#[test]
fn the_order_a_row_is_written_in_is_not_content() {
    let abilities = || [ability("ask"), ability("base")];
    let one = hash_of(
        "user",
        abilities().into_iter().chain([user(["ask", "base"])]),
    );
    let other = hash_of(
        "user",
        abilities().into_iter().chain([user(["base", "ask"])]),
    );

    assert_eq!(one, other);
}

#[test]
fn an_abilitys_name_is_content_of_what_names_it() {
    let with = |name: &str| hash_of("user", [ability(name), ability("z"), user([name, "z"])]);

    assert_eq!(with("a"), with("a"));
    // An ability is told from another by where it is declared.
    assert_ne!(with("a"), with("zz"));
}

#[test]
fn the_order_of_a_bundles_handlers_is_not_content() {
    let handler = || ty::function([], ty::int());
    let user = |first: &str, second: &str| {
        let bundle = ty::bundle([(first, handler()), (second, handler())]);
        b::function("user")
            .param("handlers", bundle)
            .body(b::null())
    };
    let hash = |first, second| hash_of("user", [ability("a"), ability("z"), user(first, second)]);

    assert_eq!(hash("a", "z"), hash("z", "a"));
}

#[test]
fn a_perform_depends_on_the_ability_and_not_on_any_handler() {
    let performer = |handler: i64| {
        let main = b::function("main").returns(ty::int()).body(b::provide(
            [("ask", answers(b::int(handler)))],
            b::call(b::name("twice"), []),
        ));
        hash_of("twice", [ability("ask"), twice(), main])
    };

    assert_eq!(performer(1), performer(2));
}
