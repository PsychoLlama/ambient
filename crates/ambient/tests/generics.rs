//! Type parameters, bounds, and generic type definitions. See
//! `spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::sync::Arc;

use ambient::builder::{self as b, pat, ty};
use ambient::core::Open;
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use support::{value, workspace};

fn check(definitions: impl IntoIterator<Item = Definition>) -> Result<Arc<Open>, Error> {
    let module = definitions
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store).check(&value("main", "main"))
}

fn returning(ret: Type, body: Expr) -> Definition {
    b::function("main").returns(ret).body(body)
}

fn t() -> Type {
    ty::named("T")
}

/// `fn id<T>(v: T): T`
fn id() -> Definition {
    b::function("id")
        .type_param("T")
        .param("v", t())
        .returns(t())
        .body(b::name("v"))
}

fn call(function: &str, args: impl IntoIterator<Item = Expr>) -> Expr {
    b::call(b::name(function), args)
}

#[test]
fn type_arguments_are_inferred_from_arguments() {
    // The argument's own type is the answer, so nothing is lost.
    assert!(check([id(), returning(ty::integer(1), call("id", [b::int(1)]))]).is_ok());
    assert!(check([id(), returning(ty::string(), call("id", [b::int(1)]))]).is_err());
}

#[test]
fn inference_looks_inside_structure() {
    let first = b::function("first")
        .type_param("A")
        .type_param("B")
        .param("p", ty::tuple([ty::named("A"), ty::named("B")]))
        .returns(ty::named("A"))
        .body(b::match_(
            b::name("p"),
            [(pat::tuple([pat::bind("a"), pat::any()]), b::name("a"))],
        ));
    let wrap = b::function("wrap")
        .type_param("T")
        .param("v", t())
        .returns(ty::list(t()))
        .body(b::list([b::name("v")]));
    let pair = b::tuple([b::int(1), b::string("a")]);

    assert!(
        check([
            first.clone(),
            returning(ty::int(), call("first", [pair.clone()]))
        ])
        .is_ok()
    );
    assert!(check([first, returning(ty::string(), call("first", [pair]))]).is_err());
    assert!(
        check([
            wrap,
            returning(ty::list(ty::int()), call("wrap", [b::int(1)]))
        ])
        .is_ok()
    );
}

#[test]
fn a_parameter_shared_by_arguments_is_their_union() {
    let either = b::function("either")
        .type_param("T")
        .param("a", t())
        .param("b", t())
        .returns(t())
        .body(b::name("a"));
    let both = call("either", [b::int(1), b::string("a")]);

    assert!(
        check([
            either.clone(),
            returning(ty::union([ty::int(), ty::string()]), both.clone())
        ])
        .is_ok()
    );
    assert!(check([either, returning(ty::int(), both)]).is_err());
}

#[test]
fn a_union_with_a_parameter_takes_what_its_other_members_do_not() {
    let or = b::function("or")
        .type_param("T")
        .param("v", ty::union([t(), ty::null()]))
        .param("fallback", t())
        .returns(t())
        .body(b::match_(
            b::name("v"),
            [
                (pat::is(ty::null()), b::name("fallback")),
                (pat::bind("found"), b::name("found")),
            ],
        ));
    let main = b::function("main")
        .param("x", ty::union([ty::int(), ty::null()]))
        .returns(ty::int())
        .body(call("or", [b::name("x"), b::int(0)]));

    assert!(check([or, main]).is_ok());
}

#[test]
fn a_type_parameter_is_opaque_inside_its_function() {
    let leak = b::function("main")
        .type_param("T")
        .param("v", t())
        .returns(ty::int())
        .body(b::name("v"));
    let forge = b::function("main")
        .type_param("T")
        .param("v", ty::int())
        .returns(t())
        .body(b::name("v"));
    let swap = b::function("main")
        .type_param("A")
        .type_param("B")
        .param("v", ty::named("A"))
        .returns(ty::named("B"))
        .body(b::name("v"));

    for definition in [leak, forge, swap] {
        assert!(matches!(check([definition]), Err(Error::Mismatch { .. })));
    }
}

fn named() -> Type {
    ty::record([("name", ty::string())])
}

#[test]
fn a_bound_is_what_the_body_knows_and_what_callers_must_meet() {
    let name_of = b::function("same")
        .bounded("T", named())
        .param("v", t())
        .returns(t())
        .body(b::let_("n", b::field(b::name("v"), "name"), b::name("v")));
    let person = b::record([("name", b::string("a")), ("age", b::int(1))]);
    let whole = ty::record([("name", ty::string()), ("age", ty::int())]);

    // The result is the argument's own type, not the bound.
    assert!(check([name_of.clone(), returning(whole, call("same", [person]))]).is_ok());
    assert!(matches!(
        check([name_of, returning(ty::unknown(), call("same", [b::int(1)]))]),
        Err(Error::CannotInfer { .. })
    ));
}

#[test]
fn an_unbounded_parameter_supports_nothing() {
    let reads = b::function("main")
        .type_param("T")
        .param("v", t())
        .returns(ty::unknown())
        .body(b::field(b::name("v"), "name"));

    assert!(matches!(check([reads]), Err(Error::NoField { .. })));
}

#[test]
fn type_arguments_may_be_written_out() {
    let at = |arg, value| returning(ty::int(), b::call(b::instantiate("id", [arg]), [value]));

    assert!(check([id(), at(ty::int(), b::int(1))]).is_ok());
    assert!(matches!(
        check([id(), at(ty::string(), b::int(1))]),
        Err(Error::Mismatch { .. })
    ));
    assert!(matches!(
        check([
            id(),
            returning(ty::unknown(), b::instantiate("id", [ty::int(), ty::int()]))
        ]),
        Err(Error::TypeArguments {
            expected: 1,
            found: 2,
            ..
        })
    ));
}

#[test]
fn a_generic_function_as_a_value_is_instantiated_by_what_is_expected() {
    let wanted = ty::function([ty::int()], ty::int());

    assert!(check([id(), returning(wanted, b::name("id"))]).is_ok());
    // Where nothing is expected of it, it stays generic.
    let kept = b::let_("f", b::name("id"), b::name("f"));
    let generic = ty::generic(
        [("U", None)],
        ty::function([ty::named("U")], ty::named("U")),
    );
    assert!(check([id(), returning(generic, kept)]).is_ok());
}

#[test]
fn a_generic_type_definition_is_transparent() {
    let boxed = b::generic_type("Box", ["T"], ty::record([("value", t())]));
    let named = returning(
        ty::apply("Box", [ty::int()]),
        b::record([("value", b::int(1))]),
    );
    let spelled = returning(
        ty::record([("value", ty::int())]),
        b::record([("value", b::int(1))]),
    );

    assert_eq!(
        check([boxed.clone(), named]).unwrap().tree,
        check([spelled]).unwrap().tree
    );
    assert!(matches!(
        check([
            boxed,
            returning(ty::apply("Box", [ty::int(), ty::int()]), b::null())
        ]),
        Err(Error::TypeArguments {
            expected: 1,
            found: 2,
            ..
        })
    ));
}

/// `type Items<T> = null | {head: T, tail: Items<T>}`
fn items() -> Definition {
    let node = ty::record([("head", t()), ("tail", ty::apply("Items", [t()]))]);
    b::generic_type("Items", ["T"], ty::union([ty::null(), node]))
}

fn items_of(member: Type) -> Type {
    ty::apply("Items", [member])
}

#[test]
fn a_recursive_generic_type_is_checked_by_structure() {
    let one = b::record([("head", b::int(1)), ("tail", b::null())]);
    let widen = b::function("main")
        .param("x", items_of(ty::integer(1)))
        .returns(items_of(ty::int()))
        .body(b::name("x"));
    let narrow = b::function("main")
        .param("x", items_of(ty::int()))
        .returns(items_of(ty::integer(1)))
        .body(b::name("x"));

    assert!(check([items(), returning(items_of(ty::int()), one.clone())]).is_ok());
    assert!(check([items(), returning(items_of(ty::string()), one)]).is_err());
    assert!(check([items(), widen]).is_ok());
    assert!(check([items(), narrow]).is_err());
}

#[test]
fn a_generic_function_builds_a_recursive_generic_type() {
    let cons = b::function("cons")
        .type_param("T")
        .param("head", t())
        .param("tail", items_of(t()))
        .returns(items_of(t()))
        .body(b::record([
            ("head", b::name("head")),
            ("tail", b::name("tail")),
        ]));
    let two = call("cons", [b::int(1), call("cons", [b::int(2), b::null()])]);

    assert!(
        check([
            items(),
            cons.clone(),
            returning(items_of(ty::int()), two.clone())
        ])
        .is_ok()
    );
    assert!(check([items(), cons, returning(items_of(ty::string()), two)]).is_err());
}

/// `type Pairs<T> = null | {head: T, next: Pairs<(T, T)>}`: each one
/// further in holds pairs of what the one before held.
fn pairs() -> Definition {
    let deeper = ty::apply("Pairs", [ty::tuple([t(), t()])]);
    let node = ty::record([("head", t()), ("next", deeper)]);
    b::generic_type("Pairs", ["T"], ty::union([ty::null(), node]))
}

fn pairs_of(member: Type) -> Type {
    ty::apply("Pairs", [member])
}

#[test]
fn a_recursive_type_may_apply_itself_to_other_types() {
    let pair = b::tuple([b::int(2), b::int(3)]);
    let inner = b::record([("head", pair), ("next", b::null())]);
    let two = b::record([("head", b::int(1)), ("next", inner)]);
    let wrong = b::record([("head", b::int(1)), ("next", two.clone())]);

    assert!(check([pairs(), returning(pairs_of(ty::int()), two)]).is_ok());
    assert!(check([pairs(), returning(pairs_of(ty::int()), wrong)]).is_err());
}

#[test]
fn uses_of_such_a_type_are_related_by_their_arguments() {
    let pass = |from: Type, to: Type| {
        let main = b::function("main")
            .param("x", pairs_of(from))
            .returns(pairs_of(to));
        check([pairs(), main.body(b::name("x"))]).is_ok()
    };
    assert!(pass(ty::integer(1), ty::int()));
    assert!(!pass(ty::int(), ty::integer(1)));
    assert!(!pass(ty::int(), ty::string()));
}

#[test]
fn two_such_types_that_differ_are_told_apart_without_unfolding_forever() {
    let triples = ty::apply("Triples", [ty::tuple([t(), t(), t()])]);
    let node = ty::record([("head", t()), ("next", triples)]);
    let other = b::generic_type("Triples", ["T"], ty::union([ty::null(), node]));
    let main = b::function("main")
        .param("x", pairs_of(ty::int()))
        .returns(ty::apply("Triples", [ty::int()]))
        .body(b::name("x"));
    assert!(check([pairs(), other, main]).is_err());
}

#[test]
fn type_parameter_names_are_not_content() {
    let identity = |param: &str| {
        b::function("main")
            .type_param(param)
            .param("v", ty::named(param))
            .returns(ty::named(param))
            .body(b::name("v"))
    };

    assert_eq!(
        check([identity("T")]).unwrap().tree.encode(),
        check([identity("Anything")]).unwrap().tree.encode(),
    );
}

/// `<U: bound>(U) -> ret`
fn generic(bound: Option<Type>, ret: Type) -> Type {
    ty::generic([("U", bound)], ty::function([ty::named("U")], ret))
}

fn u() -> Type {
    ty::named("U")
}

/// Whether a value of type `sub` is accepted where `sup` is asked for.
fn fits(sub: Type, sup: Type) -> bool {
    let pass = b::function("main").param("given", sub).returns(sup);
    check([pass.body(b::name("given"))]).is_ok()
}

#[test]
fn a_generic_function_type_is_the_same_under_other_names() {
    let renamed = ty::generic(
        [("V", None)],
        ty::function([ty::named("V")], ty::named("V")),
    );
    assert!(fits(generic(None, u()), renamed.clone()));
    assert!(fits(renamed, generic(None, u())));
}

#[test]
fn generic_function_types_are_related_as_their_function_types_are() {
    let optional = ty::union([u(), ty::null()]);
    assert!(fits(generic(None, u()), generic(None, optional.clone())));
    assert!(!fits(generic(None, optional), generic(None, u())));
}

#[test]
fn a_type_parameter_of_a_function_type_is_known_by_its_bound() {
    let bounded = || Some(ty::int());
    assert!(fits(generic(bounded(), u()), generic(bounded(), ty::int())));
    assert!(!fits(generic(None, u()), generic(None, ty::int())));
}

#[test]
fn generic_function_types_with_other_bounds_are_unrelated() {
    let (loose, tight) = (generic(None, u()), generic(Some(ty::int()), u()));
    assert!(!fits(loose.clone(), tight.clone()));
    assert!(!fits(tight, loose));
}

#[test]
fn a_generic_function_value_is_instantiated_where_a_function_is_expected() {
    let instance = ty::function([ty::int()], ty::int());
    assert!(fits(generic(None, u()), instance.clone()));
    // Nothing makes a function of one type generic.
    assert!(!fits(instance, generic(None, u())));
    // And subtyping picks no type argument: only a value is instantiated.
    let listed = |member: Type| ty::list(member);
    assert!(!fits(
        listed(generic(None, u())),
        listed(ty::function([ty::int()], ty::int()))
    ));
}

#[test]
fn a_type_parameter_is_named_only_inside_its_function_type() {
    let twice = ty::generic([("U", None)], generic(None, u()));
    let escaped = ty::tuple([Type::Bound(0)]);
    let build = |ty: Type| {
        let main = b::function("main").param("given", ty).returns(ty::null());
        b::module().define(main.body(b::null())).build()
    };
    assert!(build(generic(None, u())).is_ok());
    assert!(build(twice).is_err());
    assert!(build(escaped).is_err());
}
