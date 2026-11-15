//! Types handed to comptime functions as trees. See "Trees as values"
//! in `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use support::text::{int, text, workspace};
use support::value;

/// A function that reads a record type, and a type for it to read.
const FIELDS: &str = "
    import ambient.tree;
    type User = { name: string, age: int };
    fn fields(of: tree.Type): string {
        match of {
            let shape: { record: List<tree.Field> } => {
                return shape.record.map(fn(field) => field.name).join_with(\", \");
            }
            _ => { return \"not a record\"; }
        }
    }
";

/// What is wrong with `main` of a program written after [`FIELDS`].
fn refused(main: &str) -> Error {
    let workspace = workspace(&format!("{FIELDS} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_function_reads_the_fields_of_a_type_it_is_handed() {
    let main = "fn main(): string => comptime fields(User);";

    assert_eq!(text(&format!("{FIELDS} {main}")), "age, name");
}

#[test]
fn the_result_is_a_constant_of_the_program() {
    let main = "fn main(): \"age, name\" => comptime fields(User);";

    assert_eq!(text(&format!("{FIELDS} {main}")), "age, name");
}

#[test]
fn a_type_is_a_tree_all_the_way_down() {
    let source = "
        import ambient.tree;
        type Shape = { sides: int | null, names: List<string> };
        fn count(of: tree.Type): int {
            match of {
                let it: { record: List<tree.Field> } => {
                    return it.record.fold(1, fn(sum, field) => sum + count(field.holds));
                }
                let it: { union: List<tree.Type> } => {
                    return it.union.fold(1, fn(sum, member) => sum + count(member));
                }
                let it: { list: tree.Type } => { return 1 + count(it.list); }
                _ => { return 1; }
            }
        }
        fn main(): int => comptime count(Shape);
    ";

    // The record, a union of two, and a list of one.
    assert_eq!(int(source), 6);
}

#[test]
fn other_arguments_are_checked_as_those_of_any_call_are() {
    let main = "
        fn has(of: tree.Type, name: string): bool => fields(of).contains(name);
        fn main(): bool => comptime has(User, 1);
    ";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn only_the_name_of_a_type_is_handed_as_a_tree() {
    let main = "fn main(): string => comptime fields(1);";

    assert!(matches!(refused(main), Error::NoTree { .. }));
}

#[test]
fn a_type_that_refers_to_itself_has_no_tree() {
    let main = "
        type Chain = { next: Chain | null };
        fn main(): string => comptime fields(Chain);
    ";

    assert!(matches!(refused(main), Error::NoTree { .. }));
}

#[test]
fn a_result_is_not_reused_once_the_type_has_changed() {
    let main = "fn main(): \"age, name\" => comptime fields(User);";
    let wider = FIELDS.replace("age: int", "age: int, email: string");
    let store = MemoryStore::new();
    let check = |fields: &str| {
        let workspace = workspace(&format!("{fields} {main}"));
        Engine::new(&workspace, &store)
            .check(&value("main", "main"))
            .map(|_| ())
    };

    assert_eq!(check(FIELDS), Ok(()));
    assert!(matches!(check(&wider), Err(Error::Mismatch { .. })));
}
