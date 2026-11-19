//! Intrinsics: names the compiler knows, which a call of is a node of
//! a tree. See "Intrinsics" in `spec/definitions.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::store::MemoryStore;
use support::scripted::{app, check};
use support::text::{int, workspace};
use support::value;

/// Why `main` of a program written as `source` does not check.
fn refused(source: &str) -> String {
    let workspace = workspace(source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err().to_string()
}

#[test]
fn a_call_of_an_intrinsic_is_the_operation_it_names() {
    let source = "
        use ambient.intrinsics;
        use ambient.intrinsics.{multiply as times};
        fn main(): int => intrinsics.add(1, times(2, 3));
    ";

    assert_eq!(int(source), 7);
}

#[test]
fn a_definition_of_the_name_is_what_the_name_means() {
    let source = "
        use ambient.intrinsics;
        fn length(of: List<int>): int => 0 - intrinsics.length(of);
        fn main(): int => length([1, 2, 3]);
    ";

    assert_eq!(int(source), -3);
}

#[test]
fn an_intrinsic_is_called_and_is_no_value() {
    let source = "
        use ambient.intrinsics;
        fn main(): int => [[1]].map(intrinsics.length).length();
    ";

    assert_eq!(
        refused(source),
        "app:main.main: `intrinsics.length` is called, and is not a value"
    );
}

#[test]
fn an_error_after_a_call_of_one_is_where_it_was_written() {
    let source = "use ambient.intrinsics;\nfn one(): int {\n  let size = intrinsics.length([1]);\n  return \"one\";\n}";

    let checked = check(&app(source)).1;

    assert!(
        checked.err.starts_with("error: app/src/main.ab:4:10: "),
        "{}",
        checked.err
    );
}
