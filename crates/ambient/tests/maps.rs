//! The standard library's map, which is source like any program.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use proptest::prelude::*;
use support::text::{int, text};

/// What a function body over `d`, a map of `"b"`, `"a"`, and `"c"`
/// under 2, 1, and 3, gives.
fn over(ret: &str, body: &str) -> String {
    format!(
        r#"
        use ambient.map;
        fn main(): {ret} {{
            let d = map.of([("b", 2), ("a", 1), ("c", 3)]);
            {body}
        }}
        "#
    )
}

#[test]
fn a_map_gives_what_is_under_a_key() {
    assert_eq!(int(&over("int", r#"return d.get("b") ?? 0;"#)), 2);
    assert_eq!(int(&over("int", r#"return d.get("z") ?? 0;"#)), 0);
    assert_eq!(int(&over("int", "return d.size();")), 3);
}

#[test]
fn an_entry_tells_a_null_value_from_no_value() {
    let read = |key: &str| {
        format!(
            r#"
            use ambient.map;
            fn main(): string {{
                let d = map.of::<map.Entry<string, int | null>>([("none", null)]);
                match d.entry("{key}") {{
                    null => {{ return "absent"; }}
                    (let key, let value) => {{ return key + (value == null ? " is null" : ""); }}
                }}
            }}
            "#
        )
    };
    assert_eq!(text(&read("none")), "none is null");
    assert_eq!(text(&read("other")), "absent");
}

#[test]
fn a_map_is_read_in_the_order_of_its_keys() {
    let keys = r#"return d.keys().join_with("") + d.values().iter().fold(0, fn(a, b) => a + b).to_text();"#;

    assert_eq!(text(&over("string", keys)), "abc6");
}

#[test]
fn changing_a_map_leaves_the_one_changed_as_it_was() {
    let body = r#"
        let more = d.set(("a", 10)).remove("b").update("n", fn(seen) => ("n", (seen ?? 0) + 1));
        return more.keys().join_with("") + d.keys().join_with("");
    "#;

    assert_eq!(text(&over("string", body)), "acnabc");
}

/// A map whose entry types tie each key to the type of its value.
const PERSON: &str = r#"
    use ambient.map;
    type Person = map.Entry<"name", string> | map.Entry<"age", int>;
    fn person(): map.Map<Person> => map.of::<Person>([("name", "Ada"), ("age", 36)]);
"#;

#[test]
fn a_key_reads_the_type_of_value_its_entry_says() {
    let age = format!("{PERSON} fn main(): int => person().get(\"age\") ?? 0;");
    let name = format!("{PERSON} fn main(): string => person().get(\"name\") ?? \"\";");
    assert_eq!(int(&age), 36);
    assert_eq!(text(&name), "Ada");
}

#[test]
fn a_key_that_may_be_either_reads_either_type() {
    let source = format!(
        "{PERSON}
        fn main(): string {{
            let key: string = \"age\";
            return match person().get(key) {{
                let age: int => age.to_text(),
                let name: string => name,
                null => \"none\",
            }};
        }}"
    );
    assert_eq!(text(&source), "36");
}

#[test]
fn a_key_reads_no_value_of_another_entry() {
    let source = format!("{PERSON} fn main(): string => person().get(\"age\") ?? \"\";");
    let workspace = support::text::workspace(&source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&support::value("main", "main"));
    assert!(matches!(checked, Err(Error::Mismatch { .. })));
}

#[test]
fn keys_of_every_shape_are_read_in_their_order() {
    let source = r#"
        use ambient.map;
        fn main(): string {
            let d = map.of::<map.Entry<map.Key, string>>([
                ("b", "string"), (2, "int"), (null, "null"), (true, "bool"), (-1, "int"),
            ]);
            return d.values().join_with(" ");
        }
    "#;
    assert_eq!(text(source), "null bool int int string");
}

/// A program that sets each of `sets` and then removes each of `removes`,
/// and gives the entries left, each followed by a comma.
fn edits(sets: &[u8], removes: &[u8]) -> String {
    let set = sets
        .iter()
        .map(|key| format!(".set((\"{key:03}\", {key}))"));
    let remove = removes.iter().map(|key| format!(".remove(\"{key:03}\")"));
    let edits = set.chain(remove).collect::<Vec<_>>().concat();
    format!(
        r#"
        use ambient.map;
        fn main(): string => map.empty::<map.Entry<string, int>>(){edits}
            .entries().iter().fold("", fn(text: string, entry: (text: string, number: int)): string {{
                let (key, value) = entry;
                return text + key + "=" + value.to_text() + ",";
            }});
        "#
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn a_map_holds_what_a_map_would(
        sets in prop::collection::vec(any::<u8>(), 0..40),
        removes in prop::collection::vec(any::<u8>(), 0..40),
    ) {
        let left = sets.iter().filter(|key| !removes.contains(key));
        let left: std::collections::BTreeSet<&u8> = left.collect();
        let entry = |key: &&u8| format!("{key:03}={key},");
        let expected = left.iter().map(entry).collect::<Vec<_>>().concat();

        prop_assert_eq!(text(&edits(&sets, &removes)), expected);
    }
}
