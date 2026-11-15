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

/// A function that gives a record type with every field optional.
const PARTIAL: &str = "
    import ambient.tree;
    type User = { name: string, age: int };
    fn partial(of: tree.Type): tree.Type {
        match of {
            let shape: { record: List<tree.Field> } => {
                return { record: shape.record.map(fn(field) => {
                    name: field.name,
                    holds: { union: [field.holds, tree.nil] },
                }) };
            }
            _ => { return of; }
        }
    }
    type Draft = comptime partial(User);
";

/// What is wrong with `main` of a program written after [`PARTIAL`].
fn undrafted(main: &str) -> Error {
    let workspace = workspace(&format!("{PARTIAL} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_type_is_what_a_call_gives() {
    let main = "
        fn named(draft: Draft): string {
            match draft.name {
                null => { return \"nobody\"; }
                let name => { return name; }
            }
        }
        fn main(): string => named({ name: null, age: 3 });
    ";

    assert_eq!(text(&format!("{PARTIAL} {main}")), "nobody");
}

#[test]
fn a_derived_type_is_checked_as_one_written_out() {
    let main = "fn main(): Draft => { name: 1, age: null };";

    assert!(matches!(undrafted(main), Error::Mismatch { .. }));
}

#[test]
fn a_derived_type_has_the_hash_of_the_type_written_out() {
    let written = "type Written = { name: string | null, age: int | null };";
    let workspace = workspace(&format!("{PARTIAL} {written}"));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |name: &str| engine.hash(&support::ty("main", name)).unwrap();

    assert_eq!(hash("Draft"), hash("Written"));
}

#[test]
fn a_type_cannot_be_derived_from_itself() {
    let main = "
        type Loop = comptime partial(Loop);
        fn main(): Loop => null;
    ";

    assert!(matches!(undrafted(main), Error::ComptimeCycle(_)));
}

#[test]
fn a_derived_type_follows_the_type_it_was_derived_from() {
    let main = "fn main(): Draft => { name: null, age: null };";
    let wider = PARTIAL.replace("age: int", "age: int, email: string");
    let store = MemoryStore::new();
    let check = |types: &str| {
        let workspace = workspace(&format!("{types} {main}"));
        let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
        checked.map(|_| ())
    };

    assert_eq!(check(PARTIAL), Ok(()));
    assert!(matches!(check(&wider), Err(Error::Mismatch { .. })));
}

#[test]
fn a_derived_type_cannot_yet_name_a_brand() {
    let main = "
        brand Email;
        type Address = string & Email;
        type Same = comptime partial(Address);
        fn main(): Same => null;
    ";

    assert!(matches!(undrafted(main), Error::NotType(_)));
}
