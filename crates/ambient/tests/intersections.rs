//! Records intersected: a record that is both. See "Intersections" in
//! `docs/spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use support::text::{text, workspace};
use support::value;

const PEOPLE: &str = "
    type Named = { name: string };
    type Aged = { age: int };
    fn shown(who: Named & Aged): string => \"${who.name} is ${who.age}\";
    fn name(who: Named): string => who.name;
";

/// What is wrong with `main` of a program written as one module.
fn refused(source: &str) -> Error {
    let workspace = workspace(source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_record_that_is_both_has_the_fields_of_each() {
    let source = format!("{PEOPLE} fn main(): string => shown({{ name: \"Ada\", age: 36 }});");
    assert_eq!(text(&source), "Ada is 36");
}

#[test]
fn a_record_that_is_both_is_either() {
    let source = format!(
        "{PEOPLE}
        fn main(): string {{
            let ada: Named & Aged = {{ name: \"Ada\", age: 36 }};
            return name(ada);
        }}"
    );
    assert_eq!(text(&source), "Ada");
}

#[test]
fn a_record_of_one_is_not_both() {
    let source = format!("{PEOPLE} fn main(): string => shown({{ name: \"Ada\" }});");
    assert!(matches!(refused(&source), Error::Mismatch { .. }));
}

#[test]
fn a_field_of_both_is_both_of_its_types() {
    let source = "
        type Placed = { at: { x: int }, label: string | null };
        type Raised = { at: { y: int }, label: string };
        fn main(): string {
            let point: Placed & Raised = { at: { x: 1, y: 2 }, label: \"p\" };
            return \"${point.label}${point.at.x + point.at.y}\";
        }
    ";
    assert_eq!(text(source), "p3");
}

#[test]
fn a_field_that_cannot_be_both_makes_no_type() {
    let source = "fn main(): { x: int } & { x: string } | null => null;";
    assert!(matches!(refused(source), Error::Intersection { .. }));
}

#[test]
fn a_record_is_not_intersected_with_a_type_parameter() {
    let source = "
        fn tagged<T>(value: T & { tag: string }): string => value.tag;
        fn main(): string => tagged({ tag: \"t\" });
    ";
    let workspace = workspace(source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "tagged"));
    assert!(matches!(checked, Err(Error::Intersection { .. })));
}
