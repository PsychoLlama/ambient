//! Type matches: a type computed from another by the patterns a match
//! has. See "Type matches" in `docs/spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::Type;
use support::text::{int, workspace};
use support::value;

/// What is wrong with `main` of a program written as one module, if
/// anything is.
fn wrong(source: &str) -> Option<Error> {
    let workspace = workspace(source);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .err()
}

/// The value type of the entries whose key type a key's may match.
const VALUE: &str = "
    type Value<E, K> = match E { (K, let V) => V };
    type Entries = (key: \"a\", value: int) | (key: \"b\", value: string);
";

#[test]
fn a_key_that_matches_one_pair_gives_its_value() {
    let fits =
        format!("{VALUE} fn main(): int {{ let held: Value<Entries, \"a\"> = 1; return held; }}");
    let other = format!(
        "{VALUE} fn main(): string {{ let held: Value<Entries, \"a\"> = \"x\"; return held; }}"
    );
    assert_eq!(int(&fits), 1);
    assert!(matches!(wrong(&other), Some(Error::Mismatch { .. })));
}

#[test]
fn a_key_that_may_match_several_gives_each_value() {
    let source = format!(
        "{VALUE} fn main(): Value<Entries, string> {{ let held: Value<Entries, string> = \"x\"; return 1; }}"
    );
    assert!(wrong(&source).is_none());
}

#[test]
fn a_member_no_arm_matches_gives_nothing() {
    let source = "
        type Texts<T> = match T { let text: string => text };
        fn main(): string { let held: Texts<int | string> = \"x\"; return held; }
    ";
    let none = "
        type Texts<T> = match T { let text: string => text };
        fn main(): never { let held: Texts<int> = 1; return held; }
    ";
    assert!(wrong(source).is_none());
    assert!(matches!(wrong(none), Some(Error::Mismatch { .. })));
}

/// A generic function that gives what a type match says, proved by a
/// match with the same arms.
const PICK: &str = "
    fn pick<E: (key: string, value: unknown), K: string>(entry: E, key: K): Value<E, K> | null =>
        match entry {
            (K, let value) => value,
            _ => null,
        };
";

#[test]
fn a_match_with_the_same_arms_gives_a_type_match() {
    let source = format!(
        "{VALUE} {PICK}
        fn main(): int => pick::<Entries, \"a\">((\"a\", 1), \"a\") ?? 0;"
    );
    assert_eq!(int(&source), 1);
}

#[test]
fn a_type_match_left_open_is_given_by_nothing_else() {
    let source = format!(
        "{VALUE}
        fn pick<E: (key: string, value: unknown), K: string>(entry: E, key: K): Value<E, K> =>
            match entry {{ (_, let value) => value }};
        fn main(): int => 0;"
    );
    let workspace = workspace(&source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "pick"));
    assert!(matches!(checked, Err(Error::Mismatch { .. })));
}

#[test]
fn a_binder_of_an_open_type_is_known_by_what_holds_it() {
    let source = format!(
        "{VALUE} {PICK}
        fn length<E: (key: string, value: string), K: string>(entry: E, key: K): int =>
            match pick(entry, key) {{ null => 0, let text => text.length() }};
        fn main(): int => length::<(key: string, value: string), string>((\"a\", \"four\"), \"a\");"
    );
    assert_eq!(int(&source), 4);
}

#[test]
fn a_type_match_does_not_name_the_type_it_is_in() {
    let direct = "
        type Loop<T> = match T { let held: int => Loop<held> };
        fn main(): Loop<int> => 1;
    ";
    let through = "
        type Pair<T> = (key: T, value: Wrapped<T>);
        type Wrapped<T> = match T { let held: int => Pair<held> };
        fn main(): Wrapped<int> => 1;
    ";
    assert!(matches!(wrong(direct), Some(Error::MatchesItself(_))));
    assert!(matches!(wrong(through), Some(Error::MatchesItself(_))));
}

#[test]
fn an_arm_names_only_its_own_binders() {
    let outside = ty::matching(ty::int(), [(pat::any(), Type::Matched(0))]);
    let main = b::function("main").returns(outside).body(b::int(1));
    assert!(b::module().define(main).build().is_err());
}
