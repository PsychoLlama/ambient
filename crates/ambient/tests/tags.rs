//! Tag values. See `spec/definitions.md` and `spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::{Expr, Type};
use ambient::value::Value;
use support::{run, value, workspace};

fn evaluates(ret: Type, body: Expr) -> Value {
    run([b::function("main").returns(ret).body(body)], &[]).unwrap()
}

/// The tag `name` of the package tests are written in.
fn tag(name: &str) -> Value {
    Value::Tag {
        package: "app".to_owned(),
        tag: name.to_owned(),
    }
}

fn accepts(ret: Type, body: Expr) -> bool {
    let module = b::module().define(b::function("main").returns(ret).body(body));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .is_ok()
}

#[test]
fn a_tag_is_a_value_of_the_package_it_is_written_in() {
    assert_eq!(evaluates(ty::tag(), b::tag("missing")), tag("missing"));
}

#[test]
fn a_tag_equals_only_the_same_tag() {
    let same = b::equal(b::tag("a"), b::tag("a"));
    let other = b::equal(b::tag("a"), b::tag("b"));
    let text = b::equal(b::tag("a"), b::string("a"));

    assert_eq!(
        evaluates(ty::unknown(), b::tuple([same, other, text])),
        Value::Tuple(vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(false)
        ])
    );
}

#[test]
fn a_match_tells_tags_apart() {
    let arms = [(pat::tag("a"), b::int(1)), (pat::any(), b::int(2))];
    // Widened, so that the checker does not know which tag it is.
    let widened = b::cast(b::tag("b"), ty::tag(), b::tag("a"));
    let body = b::let_("kind", widened, b::match_(b::name("kind"), arms));

    assert_eq!(evaluates(ty::int(), body), Value::Int(2));
}

#[test]
fn a_tag_has_a_type_of_its_own_and_is_a_tag() {
    assert!(accepts(ty::tagged("a"), b::tag("a")));
    assert!(accepts(ty::tag(), b::tag("a")));
    assert!(!accepts(ty::tagged("b"), b::tag("a")));
    assert!(!accepts(ty::string(), b::tag("a")));
}

#[test]
fn a_cast_finds_a_tag_among_other_values() {
    let found = |value: Expr, to: Type| b::cast(value, to, b::string("no"));
    let results = b::tuple([
        found(b::tag("a"), ty::tag()),
        found(b::string("a"), ty::tag()),
        found(b::tag("a"), ty::tagged("b")),
    ]);

    assert_eq!(
        evaluates(ty::unknown(), results),
        Value::Tuple(vec![
            tag("a"),
            Value::String("no".to_owned()),
            Value::String("no".to_owned())
        ])
    );
}

#[test]
fn a_tag_is_scoped_by_its_package_and_not_by_its_name() {
    // The same tag text in two packages is two tags.
    let hash = |package: &str| {
        let module = b::module().define(b::function("main").returns(ty::tag()).body(b::tag("a")));
        let workspace = workspace([(package, "main", module)]);
        let store = MemoryStore::new();
        let main = ambient::symbol::ModuleId::new(package, "main").value("main");
        Engine::new(&workspace, &store).hash(&main).unwrap()
    };

    assert_ne!(hash("app"), hash("other"));
}
