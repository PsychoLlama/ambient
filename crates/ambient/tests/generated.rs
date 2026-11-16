//! Functions that are whatever tree a comptime call gives. See "Splice
//! positions" in `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use support::text::{int, workspace};
use support::value;

/// A function that makes one that multiplies, and one made with it.
const SCALED: &str = "
    import ambient.code;
    import ambient.tree;
    fn times(left: int, right: int): int => left * right;
    fn scaled(by: int): tree.Function =>
        code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
            code.call(code.definition(comptime tree.reference(times)), given.push(code.literal(by))));
    fn double = comptime scaled(2);
";

/// What is wrong with `main` of a program written after [`SCALED`].
fn refused(main: &str) -> Error {
    let workspace = workspace(&format!("{SCALED} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_function_is_what_a_call_gives() {
    let main = "fn main(): int => double(21);";

    assert_eq!(int(&format!("{SCALED} {main}")), 42);
}

#[test]
fn a_generated_function_is_called_as_its_tree_says() {
    let main = "fn main(): int => double(\"twice\");";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn a_generated_function_has_the_hash_of_the_function_written_out() {
    let written = "fn written(n: int): int => times(n, 2);";
    let workspace = workspace(&format!("{SCALED} {written}"));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |name: &str| engine.hash(&value("main", name)).unwrap();

    assert_eq!(hash("double"), hash("written"));
}

#[test]
fn a_generated_body_is_checked_against_what_the_tree_says_it_gives() {
    let main = "
        fn wrong(): tree.Function =>
            code.define([], tree.text, [], fn(given: List<code.Code>): code.Code => code.literal(1));
        fn main = comptime wrong();
    ";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn a_generated_body_nests_as_deep_as_a_written_one_may() {
    // Each call is inside the last: three hundred levels of code.
    let main = "
        fn nested(): tree.Function =>
            code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
                @range(0, 300).fold(given.first() as code.Code else code.literal(0), fn(inner: code.Code, at: int): code.Code =>
                    code.call(code.definition(comptime tree.reference(times)), [inner, code.literal(1)])));
        fn deep = comptime nested();
        fn main(): int => deep(7);
    ";

    assert_eq!(int(&format!("{SCALED} {main}")), 7);
}

#[test]
fn a_generated_function_takes_the_abilities_its_tree_lists() {
    let main = "
        ability ask(): int;
        fn asking(): tree.Function =>
            code.define([], tree.integer, [comptime tree.reference(ask)], fn(given: List<code.Code>): code.Code =>
                code.perform(comptime tree.reference(ask), []));
        fn asks = comptime asking();
        fn main(): int => provide { [ask]() => 7 } => asks();
    ";

    assert_eq!(int(&format!("{SCALED} {main}")), 7);
}

#[test]
fn an_ability_is_named_only_to_what_takes_a_reference() {
    let main = "
        ability ask(): int;
        fn main(): int => comptime times(ask, 2);
    ";

    assert!(matches!(refused(main), Error::NotValue { .. }));
}

#[test]
fn a_generated_function_names_only_what_could_be_named_there() {
    let main = "
        fn hidden(): tree.Function =>
            code.define([], tree.any, [], fn(given: List<code.Code>): code.Code =>
                code.definition({ package: \"ambient\", module: [\"map\"], name: \"locate\", kind: \"value\" }));
        fn main = comptime hidden();
    ";

    assert!(matches!(refused(main), Error::Unnameable { .. }));
}

#[test]
fn a_function_cannot_be_generated_from_itself() {
    let main = "
        fn same(of: tree.Type): tree.Function => comptime scaled(1);
        fn main = comptime same(main);
    ";

    assert!(matches!(refused(main), Error::ComptimeCycle(_)));
}

#[test]
fn a_generated_function_follows_what_made_it() {
    let store = MemoryStore::new();
    let hash = |made: &str| {
        let workspace = workspace(made);
        Engine::new(&workspace, &store).hash(&value("main", "double"))
    };
    let (twice, thrice) = (
        hash(SCALED),
        hash(&SCALED.replace("scaled(2)", "scaled(3)")),
    );

    assert!(twice.is_ok() && thrice.is_ok());
    assert_ne!(twice, thrice);
}
