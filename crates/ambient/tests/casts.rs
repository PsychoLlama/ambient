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

/// Whether `double` casts to a function from `param` to `ret`.
fn double_is(param: Type, ret: Type) -> bool {
    casts(&[double()], b::name("double"), ty::function([param], ret))
}

#[test]
fn a_function_casts_to_its_own_type() {
    assert!(double_is(ty::int(), ty::int()));
    assert!(!double_is(ty::int(), ty::string()));
    assert!(!double_is(ty::string(), ty::int()));
    assert!(!casts(&[], b::int(1), ty::function([], ty::int())));
}

#[test]
fn a_function_casts_to_a_type_its_own_is_within() {
    let maybe = || ty::union([ty::int(), ty::null()]);

    // It may be given less than it takes, and give more than is asked.
    assert!(double_is(ty::integer(1), maybe()));
    assert!(!double_is(maybe(), ty::int()));
    assert!(!double_is(ty::int(), ty::integer(2)));
    assert!(double_is(ty::int(), ty::unknown()));
}

#[test]
fn a_function_does_not_cast_to_one_of_another_arity() {
    let two = ty::function([ty::int(), ty::int()], ty::int());

    assert!(!casts(&[double()], b::name("double"), two));
    assert!(!casts(
        &[double()],
        b::name("double"),
        ty::function([], ty::int())
    ));
}

#[test]
fn a_lambda_casts_to_its_type() {
    let lambda = || b::lambda([("s", Some(ty::string()))], b::int(1));

    assert!(casts(
        &[],
        lambda(),
        ty::function([ty::string()], ty::int())
    ));
    assert!(!casts(&[], lambda(), ty::function([ty::int()], ty::int())));
}

#[test]
fn a_function_casts_only_to_a_type_that_gives_it_what_it_performs() {
    let ask = b::ability("ask", "ask").returns(ty::int());
    let asks = b::function("asks")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::perform("ask", []));
    let pure = b::function("pure").returns(ty::int()).body(b::int(1));
    let extra = [ask.into(), asks, pure];
    let with = |row: &[&str]| ty::function_row([], row.iter().copied(), ty::int());

    assert!(casts(&extra, b::name("asks"), with(&["ask"])));
    assert!(!casts(&extra, b::name("asks"), with(&[])));
    assert!(casts(&extra, b::name("pure"), with(&["ask"])));
}

#[test]
fn a_function_type_is_compared_through_the_names_in_it() {
    let pair = b::type_("Pair", ty::tuple([ty::int(), ty::int()]));
    let swap = b::function("swap")
        .param("p", ty::named("Pair"))
        .returns(ty::named("Pair"))
        .body(b::name("p"));
    let spelled = || ty::tuple([ty::int(), ty::int()]);

    assert!(casts(
        &[pair, swap],
        b::name("swap"),
        ty::function([spelled()], spelled())
    ));
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
fn a_generic_function_casts_to_its_type_at_its_type_arguments() {
    let at_int = || b::instantiate("id", [ty::int()]);
    let from = |ty: Type| ty::function([ty.clone()], ty);

    assert!(casts(&[id()], at_int(), from(ty::int())));
    assert!(!casts(&[id()], at_int(), from(ty::string())));
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
