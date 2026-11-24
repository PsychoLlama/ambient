//! Bounds on the type parameters of a type definition. See "Generics"
//! in `docs/spec/types.md`.

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
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

/// A pair of numbers.
const PAIR: &str = "type Pair<T: int> = (left: T, right: T);";

/// Whether a program of `PAIR` and `rest` is refused for an argument
/// outside a bound.
fn unbounded(rest: &str) -> bool {
    matches!(
        wrong(&format!("{PAIR} {rest}")),
        Some(Error::Mismatch { .. })
    )
}

#[test]
fn an_argument_within_its_bound_is_taken() {
    let source = format!(
        "{PAIR} fn main(): int {{ let pair: Pair<1 | 2> = (1, 2); let (left, right) = pair; return left + right; }}"
    );
    assert_eq!(int(&source), 3);
}

#[test]
fn an_argument_outside_its_bound_is_refused() {
    assert!(unbounded("fn main(): Pair<string> => (\"a\", \"b\");"));
}

#[test]
fn a_type_parameter_is_within_a_bound_by_its_own() {
    let bounded = format!(
        "{PAIR}
        fn sum<T: int>(pair: Pair<T>): int {{ let (left, right) = pair; return left + right; }}
        fn main(): int => sum((3, 4));"
    );
    assert_eq!(int(&bounded), 7);
    assert!(unbounded("fn main<T>(pair: Pair<T>): null => null;"));
}

#[test]
fn a_definition_holds_its_own_parameters_to_the_bounds_they_meet() {
    assert!(!unbounded(
        "type Twice<T: 1 | 2> = Pair<T>; fn main(): Twice<1> => (1, 1);"
    ));
    assert!(unbounded(
        "type Twice<T> = Pair<T>; fn main(): Twice<1> => (1, 1);"
    ));
}

#[test]
fn a_bound_may_name_another_parameter() {
    let within = "type Within<T, U: T> = (wide: T, narrow: U);";
    let taken = format!("{within} fn main(): Within<int, 1> => (5, 1);");
    let refused = format!("{within} fn main(): Within<1, int> => (1, 5);");
    assert!(wrong(&taken).is_none());
    assert!(matches!(wrong(&refused), Some(Error::Mismatch { .. })));
}

#[test]
fn a_parameter_of_a_generic_function_type_is_known_by_its_bound() {
    assert!(!unbounded(
        "fn main(): (fn<T: int>(pair: Pair<T>): null) => fn<T: int>(pair: Pair<T>): null => null;"
    ));
    assert!(unbounded(
        "fn main(): (fn<T>(pair: Pair<T>): null) => fn<T>(pair: Pair<T>): null => null;"
    ));
}

#[test]
fn a_map_is_only_of_entries() {
    let source = "
        use ambient.map.{self, Map};
        fn main(): Map<int> => map.empty::<(key: string, value: int)>();
    ";
    assert!(matches!(wrong(source), Some(Error::Mismatch { .. })));
}
