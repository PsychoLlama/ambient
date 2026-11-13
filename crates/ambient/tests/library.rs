//! The standard library. See `spec/modules.md` and `spec/errors.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::library;
use ambient::store::MemoryStore;
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use support::{run_in, value, workspace};

/// What `main`, with this body, gives in a workspace that has the
/// standard library.
fn evaluates(ret: Type, body: Expr) -> Value {
    let main = b::function("main").returns(ret).body(body);
    let mut workspace = workspace([("app", "main", b::module().define(main))]);
    library::install(&mut workspace).unwrap();
    run_in(&workspace).unwrap()
}

/// Which of `types`, by position, `value` is; or `-1`, if it is none of
/// them.
fn which<const N: usize>(value: Expr, types: [&str; N]) -> Expr {
    let arms = (0..)
        .zip(types)
        .map(|(position, name)| (pat::is(ty::named(name)), b::int(position)));
    b::match_(value, arms.chain([(pat::any(), b::int(-1))]))
}

#[test]
fn a_program_names_the_kind_of_a_fault() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));
    let kind = b::match_(
        b::boundary(overflow),
        [
            (
                pat::bind_as("error", pat::is(ty::named("Error"))),
                b::field(b::name("error"), "kind"),
            ),
            (pat::any(), b::tag("none")),
        ],
    );

    assert_eq!(
        evaluates(ty::int(), which(kind, ["NoMatch", "Overflow"])),
        Value::Int(1)
    );
}

#[test]
fn a_program_names_the_limit_a_budget_ran_out_of() {
    // A sum may need room for its result, and there is none.
    let limit = b::budget(b::int(1000), b::int(0), b::add(b::int(1), b::int(1)));

    assert_eq!(
        evaluates(ty::int(), which(limit, ["Steps", "Allocation"])),
        Value::Int(1)
    );
}

#[test]
fn a_tag_of_a_program_is_not_one_of_the_language() {
    // Widened, so that the checker does not know which tag it is.
    let own = b::cast(b::tag("overflow"), ty::tag(), b::tag("other"));

    assert_eq!(
        evaluates(ty::int(), which(own, ["Overflow"])),
        Value::Int(-1)
    );
}

/// The error checking `main`, with this body, gives in a workspace
/// that has the standard library.
fn refused(body: Expr) -> Error {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let mut workspace = workspace([("app", "main", b::module().define(main))]);
    library::install(&mut workspace).unwrap();
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .unwrap_err()
}

fn numbers() -> Expr {
    b::list([b::int(1), b::int(2)])
}

#[test]
fn a_shape_has_the_methods_of_its_module() {
    let lengths = b::tuple([
        b::send(numbers(), "length", []),
        b::send(b::string("abc"), "length", []),
    ]);
    let expected = Value::Tuple(vec![Value::Int(2), Value::Int(3)]);
    assert_eq!(evaluates(ty::unknown(), lengths), expected);
}

#[test]
fn a_method_call_is_the_receiver_of_another() {
    let pushed = b::send(numbers(), "push", [b::int(3)]);
    let joined = b::send(pushed, "join", [numbers()]);
    let text = b::send(b::send(joined, "length", []), "to_text", []);
    assert_eq!(evaluates(ty::string(), text), Value::String("5".into()));
}

#[test]
fn a_generic_method_takes_its_type_arguments_from_the_receiver() {
    let second = b::send(numbers(), "get", [b::int(1)]);
    assert_eq!(
        evaluates(ty::union([ty::int(), ty::null()]), second),
        Value::Int(2)
    );
}

#[test]
fn a_method_is_a_function_of_its_module_where_a_value_is_wanted() {
    let module = b::module().import("ambient.string").define(
        b::function("main")
            .returns(ty::int())
            .body(b::call(b::name("string.length"), [b::string("abcd")])),
    );
    let mut workspace = workspace([("app", "main", module)]);
    workspace.depend("app", "ambient", "ambient").unwrap();
    library::install(&mut workspace).unwrap();
    assert_eq!(run_in(&workspace).unwrap(), Value::Int(4));
}

#[test]
fn a_shape_has_no_method_its_module_lacks() {
    let missing = b::send(numbers(), "to_text", []);
    assert!(matches!(refused(missing), Error::NoMethod { name, .. } if name == "to_text"));
}

#[test]
fn a_union_of_shapes_has_no_methods_until_it_is_narrowed() {
    let either = ty::union([ty::string(), ty::list(ty::int())]);
    let narrowed = b::match_(
        b::name("either"),
        [
            (
                pat::bind_as("text", pat::is(ty::string())),
                b::send(b::name("text"), "length", []),
            ),
            (
                pat::bind_as("list", pat::any()),
                b::send(b::name("list"), "length", []),
            ),
        ],
    );
    let widen = |body| {
        b::let_(
            "either",
            b::cast(b::string("ab"), either.clone(), b::string("")),
            body,
        )
    };
    assert_eq!(evaluates(ty::int(), widen(narrowed)), Value::Int(2));
    let unnarrowed = widen(b::send(b::name("either"), "length", []));
    assert!(matches!(refused(unnarrowed), Error::NoMethod { .. }));
}
