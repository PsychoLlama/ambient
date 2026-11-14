//! Tags. See "Tags" in `spec/definitions.md` and `spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, BuildError, ModuleBuilder, pat, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::symbol::ModuleId;
use ambient::value::Value;
use support::{run, value, workspace};

/// `main`, with this body, beside the tags `a` and `b` and a type that
/// is no tag.
fn definitions(ret: Type, body: Expr) -> [Definition; 4] {
    let main = b::function("main").returns(ret).body(body);
    [b::tag("a"), b::tag("b"), b::type_("Count", ty::int()), main]
}

fn evaluates(ret: Type, body: Expr) -> Value {
    run(definitions(ret, body), &[]).unwrap()
}

/// The tag `name` of the module tests are written in.
fn tag(name: &str) -> Value {
    Value::Tag {
        package: "app".to_owned(),
        tag: format!("main.{name}"),
    }
}

fn checked(ret: Type, body: Expr) -> Result<(), Error> {
    let module = definitions(ret, body)
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.map(|_| ())
}

fn accepts(ret: Type, body: Expr) -> bool {
    checked(ret, body).is_ok()
}

/// `a`, widened so that the checker does not know which tag it is.
fn widened(name: &str) -> Expr {
    b::cast(b::name(name), ty::tag(), b::name("a"))
}

#[test]
fn naming_a_tag_gives_its_value() {
    assert_eq!(evaluates(ty::tag(), b::name("a")), tag("a"));
}

#[test]
fn a_tag_equals_only_the_same_tag() {
    let same = b::equal(b::name("a"), b::name("a"));
    let other = b::equal(widened("a"), widened("b"));
    let text = b::equal(widened("a"), b::string("main.a"));
    let expected = [true, false, false].map(Value::Bool);

    assert_eq!(
        evaluates(ty::unknown(), b::tuple([same, other, text])),
        Value::Tuple(expected.to_vec())
    );
}

#[test]
fn a_match_tells_tags_apart() {
    let arms = [
        (pat::is(ty::named("a")), b::int(1)),
        (pat::any(), b::int(2)),
    ];
    let body = b::let_("kind", widened("b"), b::match_(b::name("kind"), arms));

    assert_eq!(evaluates(ty::int(), body), Value::Int(2));
}

#[test]
fn a_tag_names_the_type_of_its_value_and_is_a_tag() {
    assert!(accepts(ty::named("a"), b::name("a")));
    assert!(accepts(ty::tag(), b::name("a")));
    assert!(!accepts(ty::named("b"), b::name("a")));
    assert!(!accepts(ty::string(), b::name("a")));
}

#[test]
fn a_cast_finds_a_tag_among_other_values() {
    let found = |value: Expr, to: Type| b::cast(value, to, b::string("no"));
    let results = b::tuple([
        found(b::name("a"), ty::tag()),
        found(b::string("a"), ty::tag()),
        found(b::name("a"), ty::named("b")),
    ]);
    let no = || Value::String("no".to_owned());

    assert_eq!(
        evaluates(ty::unknown(), results),
        Value::Tuple(vec![tag("a"), no(), no()])
    );
}

#[test]
fn a_tag_is_declared_before_it_is_named() {
    let undeclared = checked(ty::unknown(), b::name("c"));
    assert!(matches!(undeclared, Err(Error::UnknownName { .. })));
    // A type that is not a tag has no value.
    let no_tag = checked(ty::unknown(), b::name("Count"));
    assert!(matches!(no_tag, Err(Error::NotValue { .. })));
}

#[test]
fn a_tag_and_a_value_of_one_name_are_declared_once() {
    let both = b::module()
        .define(b::tag("a"))
        .define(b::function("a").body(b::null()));
    assert!(both.build().is_err());
}

/// The hash of a `main` that gives the tag `a`, declared beside it in
/// module `path` of `package`.
fn hash_in(package: &str, path: &str) -> ambient::hash::Hash {
    let main = b::function("main").returns(ty::tag()).body(b::name("a"));
    let module = b::module().define(b::tag("a")).define(main);
    let workspace = workspace([(package, path, module)]);
    let store = MemoryStore::new();
    let main = ModuleId::new(package, path).value("main");
    Engine::new(&workspace, &store).hash(&main).unwrap()
}

#[test]
fn a_tag_is_told_apart_by_the_package_and_module_that_declare_it() {
    assert_eq!(hash_in("app", "main"), hash_in("app", "main"));
    assert_ne!(hash_in("app", "main"), hash_in("other", "main"));
    assert_ne!(hash_in("app", "main"), hash_in("app", "elsewhere"));
}

#[test]
fn no_tag_is_written_by_its_text() {
    let written = ambient::literal::Literal::Tag {
        package: String::new(),
        tag: "a".to_owned(),
    };
    let main = b::function("main")
        .returns(ty::tag())
        .body(Expr::Literal(written));
    assert_eq!(
        b::module().define(main).build().err(),
        Some(BuildError::WrittenTag("a".to_owned()))
    );
}
