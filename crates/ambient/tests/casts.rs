//! Casts that compare types at run time: to a function type, to a cell
//! type, and to a type parameter. See `spec/backends.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::run;

/// `fn hide(v: unknown): unknown`, so that a cast has something to do.
fn hide() -> Definition {
    b::function("hide")
        .param("v", ty::unknown())
        .returns(ty::unknown())
        .body(b::name("v"))
}

/// Whether `cast`, which is the string `"no"` if it does not pass,
/// passes.
fn passes(cast: Expr) -> Expr {
    b::match_(
        cast,
        [
            (pat::string("no"), b::boolean(false)),
            (pat::any(), b::boolean(true)),
        ],
    )
}

/// Whether a value, seen as `unknown`, casts to `ty`, beside `extra`.
fn casts(extra: &[Definition], value: Expr, ty: Type) -> bool {
    let hidden = b::call(b::name("hide"), [value]);
    let body = passes(b::cast(hidden, ty, b::string("no")));
    let main = b::function("main").returns(ty::bool()).body(body);
    let mut definitions = extra.to_vec();
    definitions.extend([hide(), main]);
    run(definitions, &[]).unwrap() == Value::Bool(true)
}

/// `fn double(n: int): int`
fn double() -> Definition {
    b::function("double")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::name("n")))
}

/// Whether checking refuses a cast to `ty` because no value can be
/// tested for it.
fn refused(ty: Type) -> bool {
    use ambient::engine::Engine;
    use ambient::error::Error;
    use ambient::store::MemoryStore;

    let function = || ty::function([ty::int()], ty::int());
    let link = ty::record([("next", ty::named("Chain")), ("run", function())]);
    let chain = b::type_("Chain", ty::union([ty::null(), link]));
    let main = b::function("main")
        .param("v", ty::unknown())
        .returns(ty::unknown())
        .body(b::cast(b::name("v"), ty, b::null()));
    let module = b::module().define(chain).define(main);
    let workspace = support::workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&support::value("main", "main"));
    matches!(checked, Err(Error::Untestable { .. }))
}

#[test]
fn a_cast_to_a_type_that_holds_a_function_type_is_refused() {
    let function = || ty::function([ty::int()], ty::int());

    assert!(refused(function()));
    assert!(refused(ty::record([("f", function())])));
    assert!(refused(ty::list(ty::union([function(), ty::null()]))));
    assert!(refused(ty::named("Chain")));
    assert!(!refused(ty::list(ty::int())));
}

#[test]
fn a_cast_to_a_cell_of_functions_is_allowed() {
    // A cell keeps the type it was made at, whatever that holds.
    assert!(!refused(ty::cell(ty::function([ty::int()], ty::int()))));
}

#[test]
fn a_cell_of_functions_casts_to_a_cell_of_the_same_functions() {
    let held = || ty::function([ty::int()], ty::int());
    let cell = || b::cell(held(), b::name("double"));

    assert!(casts(&[double()], cell(), ty::cell(held())));
    // A cell holds exactly its type, not one its type is within.
    let wider = ty::function([ty::integer(1)], ty::int());
    assert!(!casts(&[double()], cell(), ty::cell(wider)));
}

fn t() -> Type {
    ty::named("T")
}

/// `T`, or the string a cast that does not pass gives.
fn kept(ty: Type) -> Type {
    ty::union([ty, ty::text("no")])
}

/// `fn keep<T>(v: unknown): T | "no"`, which casts `v` to `T`.
fn keep() -> Definition {
    b::function("keep")
        .type_param("T")
        .param("v", ty::unknown())
        .returns(kept(t()))
        .body(b::cast(b::name("v"), t(), b::string("no")))
}

/// Whether `main`, which gives what a cast gave, gives a value that
/// passed.
fn passed(extra: &[Definition], result: Expr) -> bool {
    let main = b::function("main").returns(ty::bool()).body(passes(result));
    let mut definitions = extra.to_vec();
    definitions.extend([keep(), main]);
    run(definitions, &[]).unwrap() == Value::Bool(true)
}

/// Whether `generic`, used at `int`, keeps `value`.
fn keeps(extra: &[Definition], generic: &str, value: Expr) -> bool {
    let call = b::call(b::instantiate(generic, [ty::int()]), [value]);
    passed(extra, call)
}

#[test]
fn a_cast_to_a_type_parameter_tests_against_the_type_argument() {
    assert!(keeps(&[], "keep", b::int(1)));
    assert!(!keeps(&[], "keep", b::string("a")));
}

#[test]
fn a_generic_function_hands_its_type_arguments_on() {
    let through = |name: &str, arg: Type, ret: Type| {
        b::function(name)
            .type_param("T")
            .param("v", ty::unknown())
            .returns(kept(ret))
            .body(b::call(b::instantiate("keep", [arg]), [b::name("v")]))
    };
    let extra = [
        through("same", t(), t()),
        through("listed", ty::list(t()), ty::list(t())),
    ];

    assert!(keeps(&extra, "same", b::int(1)));
    assert!(!keeps(&extra, "same", b::list([b::int(1)])));
    assert!(keeps(&extra, "listed", b::list([b::int(1)])));
    assert!(!keeps(&extra, "listed", b::list([b::string("a")])));
}

#[test]
fn a_function_that_calls_itself_keeps_its_type_arguments() {
    // `fn after<T>(v: unknown, n: int): T | "no"` casts once `n` is zero.
    let again = b::call(
        b::instantiate("after", [t()]),
        [b::name("v"), b::subtract(b::name("n"), b::int(1))],
    );
    let after = b::function("after")
        .type_param("T")
        .param("v", ty::unknown())
        .param("n", ty::int())
        .returns(kept(t()))
        .body(b::match_(
            b::name("n"),
            [
                (pat::int(0), b::cast(b::name("v"), t(), b::string("no"))),
                (pat::any(), again),
            ],
        ));
    let at = |value| b::call(b::instantiate("after", [ty::int()]), [value, b::int(50)]);

    assert!(passed(std::slice::from_ref(&after), at(b::int(1))));
    assert!(!passed(&[after], at(b::string("a"))));
}

#[test]
fn a_lambda_in_a_generic_function_knows_its_type_arguments() {
    let later = b::function("later")
        .type_param("T")
        .param("v", ty::unknown())
        .returns(ty::function([], kept(t())))
        .body(b::lambda([], b::cast(b::name("v"), t(), b::string("no"))));
    let at = |value| b::call(b::call(b::instantiate("later", [ty::int()]), [value]), []);

    assert!(passed(std::slice::from_ref(&later), at(b::int(1))));
    assert!(!passed(&[later], at(b::string("a"))));
}

/// `fn fresh<T>(v: T): cell<T>`
fn fresh() -> Definition {
    b::function("fresh")
        .type_param("T")
        .param("v", t())
        .returns(ty::cell(t()))
        .body(b::cell(t(), b::name("v")))
}

#[test]
fn a_cell_made_at_a_type_parameter_holds_the_type_argument() {
    let cell = || b::call(b::instantiate("fresh", [ty::int()]), [b::int(1)]);

    assert!(casts(&[fresh()], cell(), ty::cell(ty::int())));
    assert!(!casts(&[fresh()], cell(), ty::cell(ty::string())));
    assert!(!casts(&[fresh()], cell(), ty::cell(ty::integer(1))));
}

/// `fn id<T>(v: T): T`
fn id() -> Definition {
    b::function("id")
        .type_param("T")
        .param("v", t())
        .returns(t())
        .body(b::name("v"))
}

#[test]
fn no_function_passes_a_cast_to_a_type_parameter() {
    // The type argument is a function type, which no value says it is.
    let double_type = ty::function([ty::int()], ty::int());
    let call = b::call(b::instantiate("keep", [double_type]), [b::name("double")]);

    assert!(!passed(&[double()], call));
}

#[test]
fn a_generic_function_equals_itself_at_the_same_type_arguments() {
    let equal = |left: Type, right: Type| {
        let at = |ty| b::call(b::name("hide"), [b::instantiate("id", [ty])]);
        let main = b::function("main")
            .returns(ty::bool())
            .body(b::equal(at(left), at(right)));
        run([id(), hide(), main], &[]).unwrap() == Value::Bool(true)
    };
    let pair = || ty::tuple([ty::int(), ty::int()]);

    assert!(equal(pair(), pair()));
    assert!(!equal(ty::int(), ty::string()));
}
