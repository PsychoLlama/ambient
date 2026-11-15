//! The standard library's map, which is source like any program.

#![allow(clippy::unwrap_used)]

mod support;

use proptest::prelude::*;
use support::text::{int, text};

/// What a function body over `d`, a map of `"b"`, `"a"`, and `"c"`
/// under 2, 1, and 3, gives.
fn over(ret: &str, body: &str) -> String {
    format!(
        r#"
        import ambient.map;
        fn main(): {ret} {{
            let d = map.of([("b", 2), ("a", 1), ("c", 3)]);
            {body}
        }}
        "#
    )
}

#[test]
fn a_map_gives_what_is_under_a_key() {
    assert_eq!(int(&over("int", r#"return d.get("b") as int else 0;"#)), 2);
    assert_eq!(int(&over("int", r#"return d.get("z") as int else 0;"#)), 0);
    assert_eq!(int(&over("int", "return d.size();")), 3);
}

#[test]
fn an_entry_tells_a_null_value_from_no_value() {
    let read = |key: &str| {
        format!(
            r#"
            import ambient.map;
            fn main(): string {{
                let d = map.of::<int | null>([("none", null)]);
                match d.entry("{key}") {{
                    null => {{ return "absent"; }}
                    (let key, let value) => {{ return key ++ (value == null ? " is null" : ""); }}
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
    let keys = r#"return d.keys().join_with("") ++ d.values().iter().fold(0, fn(a, b) => a + b).to_text();"#;

    assert_eq!(text(&over("string", keys)), "abc6");
}

#[test]
fn changing_a_map_leaves_the_one_changed_as_it_was() {
    let body = r#"
        let more = d.set("a", 10).remove("b").update("n", fn(seen) => (seen as int else 0) + 1);
        return more.keys().join_with("") ++ d.keys().join_with("");
    "#;

    assert_eq!(text(&over("string", body)), "acnabc");
}

/// A program that sets each of `sets` and then removes each of `removes`,
/// and gives the entries left, each followed by a comma.
fn edits(sets: &[u8], removes: &[u8]) -> String {
    let set = sets.iter().map(|key| format!(".set(\"{key:03}\", {key})"));
    let remove = removes.iter().map(|key| format!(".remove(\"{key:03}\")"));
    let edits = set.chain(remove).collect::<Vec<_>>().concat();
    format!(
        r#"
        import ambient.map;
        fn main(): string => map.empty::<int>(){edits}
            .entries().iter().fold("", fn(text: string, entry: (string, int)): string {{
                let (key, value) = entry;
                return text ++ key ++ "=" ++ value.to_text() ++ ",";
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
