//! The type checker. See `spec/types.md`.

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

/// Checks `main` among `definitions`.
fn check(definitions: impl IntoIterator<Item = Definition>) -> Result<Arc<Open>, Error> {
    let module = definitions
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store).check(&value("main", "main"))
}

/// `main` with no parameters.
fn returning(ret: Type, body: Expr) -> Definition {
    b::function("main").returns(ret).body(body)
}

/// `main` with one parameter, `x`.
fn taking(param: Type, ret: Type, body: Expr) -> Definition {
    b::function("main")
        .param("x", param)
        .returns(ret)
        .body(body)
}

fn accepts(definition: Definition) -> bool {
    check([definition]).is_ok()
}

fn x() -> Expr {
    b::name("x")
}

fn mismatch(result: Result<Arc<Open>, Error>) -> (String, String) {
    match result {
        Err(Error::Mismatch {
            expected, found, ..
        }) => (expected, found),
        other => panic!("expected a mismatch, got {other:?}"),
    }
}

// Subtyping

#[test]
fn a_literal_has_its_own_type_and_widens_only_by_subtyping() {
    assert!(accepts(returning(ty::integer(1), b::int(1))));
    assert!(accepts(returning(ty::int(), b::int(1))));

    let (expected, found) = mismatch(check([returning(ty::integer(2), b::int(1))]));
    assert_eq!((expected.as_str(), found.as_str()), ("2", "1"));
}

#[test]
fn int_and_float_are_unrelated() {
    assert!(!accepts(returning(ty::float(), b::int(1))));
    assert!(!accepts(returning(ty::int(), b::float(1.0))));
}

#[test]
fn a_record_is_a_subtype_of_one_with_fewer_or_wider_fields() {
    let point = b::record([("x", b::int(1)), ("y", b::int(2))]);

    assert!(accepts(returning(
        ty::record([("x", ty::int())]),
        point.clone()
    )));
    assert!(accepts(returning(ty::record([]), point.clone())));
    assert!(!accepts(returning(
        ty::record([("z", ty::int())]),
        point.clone()
    )));
    assert!(!accepts(returning(
        ty::record([("x", ty::string())]),
        point
    )));
}

#[test]
fn a_tuple_is_a_subtype_of_a_list_of_its_members() {
    let pair = b::tuple([b::int(1), b::string("a")]);
    let either = ty::union([ty::int(), ty::string()]);

    assert!(accepts(returning(ty::list(either), pair.clone())));
    assert!(!accepts(returning(ty::list(ty::int()), pair.clone())));
    assert!(!accepts(returning(ty::tuple([ty::int()]), pair)));
}

#[test]
fn lists_are_covariant() {
    let numbers = b::list([b::int(1), b::int(2)]);

    assert!(accepts(returning(ty::list(ty::int()), numbers.clone())));
    assert!(accepts(returning(ty::list(ty::unknown()), numbers.clone())));
    assert!(!accepts(returning(ty::list(ty::string()), numbers)));
    assert!(accepts(returning(ty::list(ty::never()), b::list([]))));
}

#[test]
fn a_function_is_a_subtype_of_one_with_narrower_parameters_or_a_wider_result() {
    let helper = |param, ret, body| {
        b::function("helper")
            .param("v", param)
            .returns(ret)
            .body(body)
    };
    let wide = helper(
        ty::union([ty::int(), ty::string()]),
        ty::integer(1),
        b::int(1),
    );
    let narrow = helper(ty::int(), ty::int(), b::int(1));
    let wants = |param, ret| returning(ty::function([param], ret), b::name("helper"));

    assert!(check([wide.clone(), wants(ty::int(), ty::int())]).is_ok());
    assert!(check([narrow.clone(), wants(ty::int(), ty::int())]).is_ok());
    // A parameter may not be narrower than what callers will pass.
    assert!(check([narrow.clone(), wants(ty::unknown(), ty::int())]).is_err());
    // A result may not be wider than what callers expect.
    assert!(check([narrow, wants(ty::int(), ty::integer(1))]).is_err());
}

#[test]
fn never_is_below_everything_and_unknown_above() {
    assert!(accepts(taking(ty::never(), ty::int(), x())));
    assert!(accepts(taking(ty::int(), ty::unknown(), x())));
    assert!(!accepts(taking(ty::unknown(), ty::int(), x())));
}

#[test]
fn a_union_accepts_each_member_and_is_accepted_where_all_are() {
    let either = || ty::union([ty::int(), ty::null()]);

    assert!(accepts(returning(either(), b::null())));
    assert!(accepts(returning(either(), b::int(1))));
    assert!(!accepts(returning(either(), b::string("a"))));
    assert!(!accepts(taking(either(), ty::int(), x())));
    assert!(accepts(taking(
        ty::bool(),
        ty::union([ty::boolean(true), ty::boolean(false)]),
        x()
    )));
}

// Type definitions

#[test]
fn a_non_recursive_type_definition_is_transparent() {
    let point = b::type_("Point", ty::record([("x", ty::int())]));
    let named = returning(ty::named("Point"), b::record([("x", b::int(1))]));
    let spelled = returning(
        ty::record([("x", ty::int())]),
        b::record([("x", b::int(1))]),
    );

    let named = check([point, named]).unwrap();
    let spelled = check([spelled]).unwrap();

    assert_eq!(named.tree, spelled.tree);
    assert!(named.slots.is_empty());
}

fn list_of_int(name: &str) -> Definition {
    let node = ty::record([("head", ty::int()), ("tail", ty::named(name))]);
    b::type_(name, ty::union([ty::null(), node]))
}

#[test]
fn a_recursive_type_describes_values_of_any_depth() {
    let two = b::record([
        ("head", b::int(1)),
        (
            "tail",
            b::record([("head", b::int(2)), ("tail", b::null())]),
        ),
    ]);
    let bad = b::record([("head", b::int(1)), ("tail", b::int(2))]);

    assert!(check([list_of_int("Items"), returning(ty::named("Items"), two)]).is_ok());
    assert!(check([list_of_int("Items"), returning(ty::named("Items"), bad)]).is_err());
}

#[test]
fn recursive_types_are_compared_by_structure() {
    let convert = taking(ty::named("A"), ty::named("B"), x());

    assert!(check([list_of_int("A"), list_of_int("B"), convert]).is_ok());
}

#[test]
fn a_recursive_type_must_pass_through_a_constructor() {
    for body in [ty::named("T"), ty::union([ty::null(), ty::named("T")])] {
        let result = check([
            b::type_("T", body),
            taking(ty::named("T"), ty::unknown(), x()),
        ]);
        assert!(matches!(result, Err(Error::Uncontractive(_))), "{result:?}");
    }
}

// Expressions

#[test]
fn a_field_must_exist_in_every_member_of_a_union() {
    let both = ty::union([
        ty::record([("v", ty::int())]),
        ty::record([("v", ty::string())]),
    ]);
    let one = ty::union([ty::record([("v", ty::int())]), ty::record([])]);
    let v = || b::field(x(), "v");

    assert!(accepts(taking(
        both,
        ty::union([ty::int(), ty::string()]),
        v()
    )));
    assert!(matches!(
        check([taking(one, ty::unknown(), v())]),
        Err(Error::NoField { .. })
    ));
}

#[test]
fn operators_are_defined_per_primitive() {
    assert!(accepts(returning(ty::int(), b::add(b::int(1), b::int(2)))));
    assert!(accepts(returning(
        ty::float(),
        b::multiply(b::float(1.0), b::float(2.0))
    )));
    assert!(accepts(returning(
        ty::bool(),
        b::less(b::int(1), b::int(2))
    )));
    assert!(accepts(returning(
        ty::bool(),
        b::equal(b::int(1), b::string("a"))
    )));
    assert!(matches!(
        check([returning(ty::unknown(), b::add(b::int(1), b::float(2.0)))]),
        Err(Error::Operands { .. })
    ));
}

#[test]
fn arguments_are_checked_against_parameters() {
    let helper = b::function("helper")
        .param("v", ty::int())
        .returns(ty::int())
        .body(b::name("v"));
    let call = |arg| returning(ty::int(), b::call(b::name("helper"), [arg]));

    assert!(check([helper.clone(), call(b::int(1))]).is_ok());
    assert!(matches!(
        check([helper, call(b::string("a"))]),
        Err(Error::Mismatch { .. })
    ));
    assert!(matches!(
        check([returning(ty::unknown(), b::call(b::int(1), []))]),
        Err(Error::NotCallable { .. })
    ));
}

#[test]
fn a_binding_has_the_type_of_its_value() {
    let body = b::let_("n", b::int(2), b::add(b::name("n"), b::name("n")));

    assert!(accepts(returning(ty::int(), body)));
}

#[test]
fn a_cast_yields_the_target_type_or_the_alternative() {
    assert!(accepts(taking(
        ty::unknown(),
        ty::int(),
        b::cast(x(), ty::int(), b::int(0))
    )));
    assert!(accepts(taking(
        ty::unknown(),
        ty::union([ty::string(), ty::null()]),
        b::cast(x(), ty::string(), b::null()),
    )));
    assert!(!accepts(taking(
        ty::unknown(),
        ty::string(),
        b::cast(x(), ty::string(), b::null())
    )));
}

// Match

#[test]
fn a_match_is_exhaustive() {
    let on_bool = |arms| taking(ty::bool(), ty::int(), b::match_(x(), arms));

    assert!(accepts(on_bool(vec![
        (pat::boolean(true), b::int(1)),
        (pat::boolean(false), b::int(0))
    ])));
    match check([on_bool(vec![(pat::boolean(true), b::int(1))])]) {
        Err(Error::NotExhaustive { remaining, .. }) => assert_eq!(remaining, "false"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        check([taking(
            ty::int(),
            ty::int(),
            b::match_(x(), [(pat::int(0), b::int(1))])
        )]),
        Err(Error::NotExhaustive { .. })
    ));
}

#[test]
fn an_arm_that_cannot_be_taken_is_an_error() {
    let after_wildcard = b::match_(x(), [(pat::any(), b::int(1)), (pat::int(0), b::int(2))]);
    let wrong_shape = b::match_(
        x(),
        [(pat::string("a"), b::int(1)), (pat::any(), b::int(2))],
    );

    assert!(matches!(
        check([taking(ty::int(), ty::int(), after_wildcard)]),
        Err(Error::UnreachableArm { arm: 1, .. })
    ));
    assert!(matches!(
        check([taking(ty::int(), ty::int(), wrong_shape)]),
        Err(Error::UnreachableArm { arm: 0, .. })
    ));
}

fn shape() -> Type {
    ty::union([
        ty::record([("kind", ty::text("circle")), ("radius", ty::float())]),
        ty::record([("kind", ty::text("square")), ("side", ty::float())]),
    ])
}

#[test]
fn a_match_narrows_a_union_of_records_by_a_literal_field() {
    let circle = (
        pat::record([("kind", pat::string("circle")), ("radius", pat::bind("r"))]),
        b::multiply(b::name("r"), b::name("r")),
    );
    let square = (
        pat::bind_as("s", pat::record([("kind", pat::string("square"))])),
        b::field(b::name("s"), "side"),
    );

    let both = b::match_(x(), [circle.clone(), square]);
    assert!(accepts(taking(shape(), ty::float(), both)));

    let result = check([taking(shape(), ty::float(), b::match_(x(), [circle]))]);
    match result {
        Err(Error::NotExhaustive { remaining, .. }) => {
            assert_eq!(remaining, "{kind: \"square\", side: float}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_match_narrows_one_field_of_a_record() {
    let subject = ty::record([("kind", ty::union([ty::text("a"), ty::text("b")]))]);
    let arm = |kind, result| (pat::record([("kind", pat::string(kind))]), b::int(result));

    assert!(accepts(taking(
        subject.clone(),
        ty::int(),
        b::match_(x(), [arm("a", 1), arm("b", 2)])
    )));
    assert!(!accepts(taking(
        subject,
        ty::int(),
        b::match_(x(), [arm("a", 1)])
    )));
}

#[test]
fn a_type_test_narrows_the_subject_and_what_is_left() {
    let body = b::match_(
        x(),
        [
            (
                pat::bind_as("n", pat::is(ty::int())),
                b::add(b::name("n"), b::int(1)),
            ),
            // Only `string` is left, so `s` is a string without a test.
            (
                pat::bind("s"),
                b::match_(
                    b::equal(b::name("s"), b::string("")),
                    [(pat::any(), b::int(0))],
                ),
            ),
        ],
    );

    assert!(accepts(taking(
        ty::union([ty::int(), ty::string()]),
        ty::int(),
        body
    )));
}

#[test]
fn unknown_is_narrowed_by_shape() {
    let body = b::match_(
        x(),
        [
            (
                pat::record([("name", pat::bind_as("n", pat::is(ty::string())))]),
                b::name("n"),
            ),
            (pat::any(), b::string("")),
        ],
    );

    assert!(accepts(taking(ty::unknown(), ty::string(), body)));
}

#[test]
fn a_tuple_pattern_matches_a_list_of_that_length() {
    let body = b::match_(
        x(),
        [
            (
                pat::tuple([pat::bind("a"), pat::bind("b")]),
                b::add(b::name("a"), b::name("b")),
            ),
            (pat::any(), b::int(0)),
        ],
    );

    assert!(accepts(taking(
        ty::list(ty::int()),
        ty::int(),
        body.clone()
    )));
    assert!(accepts(taking(
        ty::tuple([ty::int(), ty::int()]),
        ty::int(),
        b::match_(
            x(),
            [(
                pat::tuple([pat::bind("a"), pat::bind("b")]),
                b::add(b::name("a"), b::name("b"))
            )],
        )
    )));
    assert!(!accepts(taking(ty::list(ty::string()), ty::int(), body)));
}

#[test]
fn a_match_has_the_union_of_its_arms() {
    let body = b::match_(
        x(),
        [
            (pat::boolean(true), b::int(1)),
            (pat::any(), b::string("no")),
        ],
    );

    assert!(accepts(taking(
        ty::bool(),
        ty::union([ty::int(), ty::string()]),
        body.clone()
    )));
    assert!(!accepts(taking(ty::bool(), ty::int(), body)));
}

// Hashing

#[test]
fn union_member_order_is_not_content() {
    let one = check([taking(
        ty::union([ty::int(), ty::string()]),
        ty::null(),
        b::null(),
    )])
    .unwrap();
    let other = check([taking(
        ty::union([ty::string(), ty::int()]),
        ty::null(),
        b::null(),
    )])
    .unwrap();

    assert_eq!(one.tree.encode(), other.tree.encode());
}
