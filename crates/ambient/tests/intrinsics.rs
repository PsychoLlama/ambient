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

#[test]
fn a_cast_takes_a_value_and_what_to_give_otherwise() {
    assert_eq!(
        refused("fn main(value: unknown): int | null => cast::<int>(value);"),
        "app:main.main: `cast` is written `cast::<T>(value, fallback)`"
    );
}

#[test]
fn an_error_in_a_cast_that_holds_its_fallback_is_at_the_call() {
    let source = "fn main(value: unknown): string =>\n  cast::<int>(value, [1].length());";

    let checked = check(&app(source)).1;

    assert!(
        checked.err.starts_with("error: app/src/main.ab:2:3: "),
        "{}",
        checked.err
    );
}

#[test]
fn a_construct_takes_its_body_as_a_function_written_in_place() {
    let written = "fn main(): int | Error => contain(fn() => 1 / 1);";
    let handed = "fn one(): int => 1;\nfn main(): int | Error => contain(one);";

    assert_eq!(int(written), 1);
    assert_eq!(
        refused(handed),
        "app:main.main: `contain` takes a body: `fn()` and what it gives, written in place"
    );
}

#[test]
fn a_cell_holds_what_its_first_value_is_unless_it_says() {
    let source = "
        fn main(): int {
            let count = cell(1);
            let either = cell::<int | null>(null);
            count.update(fn(held) => held + 1);
            either.update(fn(_) => 40);
            return count.read() + (cast::<int>(either.read(), 0));
        }
    ";

    assert_eq!(int(source), 42);
}

#[test]
fn a_function_of_the_program_hides_an_intrinsic_of_its_name() {
    let source = "
        use ambient.tasks.{first};
        fn all(left: int, right: int): int => left + right;
        fn cell(of: int): int => of * 2;
        fn main(): int => all(cell(1), first(fn() => 3));
    ";

    assert_eq!(int(source), 5);
}

#[test]
fn a_task_is_still_started_only_in_a_group() {
    let source = "use ambient.tasks;\nfn main(): null => tasks.spawn(fn() => null);";

    assert_eq!(
        refused(source),
        "app:main.main: a spawn must be written in the body of a group"
    );
}

#[test]
fn a_plus_takes_two_of_one_shape() {
    let refused = refused("fn main(): unknown => [1] + 0x[02];");

    assert!(refused.contains("`+` is not defined for"), "{refused}");
}

#[test]
fn what_is_never_null_has_nothing_to_fall_back_from() {
    let never = refused("fn main(): int => 1 ?? 2;");
    let always = refused("fn main(): int => null ?? 2;");

    assert!(
        never.ends_with("`??` follows a value that is never null"),
        "{never}"
    );
    assert!(
        always.ends_with("`??` follows a value that is always null"),
        "{always}"
    );
}

#[test]
fn a_method_and_a_generic_function_are_shown_as_they_are_written() {
    let method = refused(
        "fn main(counter: { count: int, add(self: { count: int }, times: int): int with throw }): int => counter;",
    );
    let generic =
        refused("fn main(pick: fn<T, U: { count: int }>(one: T, other: U): T): int => pick;");

    assert!(
        method.contains("`{add(self: {count: int}, times: int): int with throw, count: int}`"),
        "{method}"
    );
    assert!(
        generic.contains("`fn<G0, G1: {count: int}>(one: G0, other: G1): G0`"),
        "{generic}"
    );
}

#[test]
fn a_type_is_shown_with_the_names_it_was_written_with() {
    let tuple = refused("fn main(): (name: string, count: int) => 1;");
    let function = refused("fn main(): fn(note: string): null with throw | null => 1;");

    assert!(tuple.contains("`(name: string, count: int)`"), "{tuple}");
    assert!(
        function.contains("`null | (fn(note: string): null with throw)`"),
        "{function}"
    );
}
