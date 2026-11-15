//! The standard library's JSON, which is source like any program. See
//! `spec/json.md`.

#![allow(clippy::unwrap_used)]

mod support;

use proptest::prelude::*;
use support::text::{int, text};

/// A string literal of the language that holds `written`.
fn literal(written: &str) -> String {
    let escaped = written
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// What `json.write(json.parse(text))` gives for each text, or the
/// message of the error that reading it threw. One program reads them
/// all.
fn rewritten<const N: usize>(written: [&str; N]) -> [String; N] {
    let texts: Vec<String> = written.into_iter().map(literal).collect();
    let source = format!(
        r#"
        import ambient.json;
        import ambient.{{catch, Error}};

        fn rewrite(text: string): string {{
            match catch(fn() => json.write(json.parse(text))) {{
                let error: Error => {{ return error.message; }}
                let written: string => {{ return written; }}
            }}
        }}
        fn main(): string => [{}].map(rewrite).join_with("\u{{1}}");
        "#,
        texts.join(", ")
    );
    let all = text(&source);
    let mut each = all.split('\u{1}').map(str::to_owned);
    std::array::from_fn(|_| each.next().unwrap())
}

#[test]
fn a_value_is_written_as_it_was_read() {
    let nested = r#"{"a":[1,-2.5,true,false,null],"b":{"c":"d"},"e":[]}"#;

    assert_eq!(
        rewritten([nested, " [ 1 , 2 ] ", "{ }", "\"\""]),
        [nested, "[1,2]", "{}", "\"\""]
    );
}

#[test]
fn an_object_is_written_in_the_order_of_its_names() {
    // A name written twice holds what it was given last.
    assert_eq!(rewritten([r#"{"b":1,"a":2,"b":3}"#]), [r#"{"a":2,"b":3}"#]);
}

#[test]
fn a_number_without_a_fraction_is_an_integer_if_it_can_be() {
    let [small, wide, float, exponent] = rewritten(["-12", "9223372036854775808", "1.50", "1E2"]);

    assert_eq!(small, "-12");
    // Past the last integer, the nearest float.
    assert_eq!(wide, "9.223372036854776e18");
    assert_eq!(float, "1.5");
    assert_eq!(exponent, "100.0");
}

#[test]
fn an_escape_is_read_as_the_character_it_names() {
    let [plain, named, halves, written] =
        rewritten([r#""a\"b\\c\/d\n\t""#, r#""éA""#, r#""😀""#, "\"é😀\""]);

    assert_eq!(plain, r#""a\"b\\c/d\n\t""#);
    assert_eq!(named, "\"éA\"");
    assert_eq!(halves, "\"😀\"");
    assert_eq!(written, "\"é😀\"");
}

#[test]
fn a_control_character_is_written_as_an_escape() {
    assert_eq!(
        rewritten([r#""\u0001\b\f\r""#]),
        [r#""\u0001\u0008\u000c\r""#]
    );
}

#[test]
fn text_that_is_not_json_says_where_it_stops_being() {
    let wrong = rewritten([
        "",
        "[1, 2",
        "{\"a\" 1}",
        "{\n  \"a\": tru\n}",
        "01",
        "1.",
        "[1] 2",
        "\"a",
        r#""\ud83d""#,
        r#""\x""#,
        "{a: 1}",
    ]);

    assert_eq!(
        wrong,
        [
            "expected a value at line 1, column 1",
            "expected `,` or `]` at line 1, column 6",
            "expected `:` at line 1, column 6",
            "expected a value at line 2, column 8",
            "expected a number that does not begin with zero at line 1, column 1",
            "expected a digit at line 1, column 3",
            "expected the end at line 1, column 5",
            "expected `\"` at line 1, column 3",
            "expected an escape that names a character at line 1, column 3",
            "expected an escape at line 1, column 3",
            "expected a name at line 1, column 2",
        ]
    );
}

#[test]
fn a_member_is_reached_by_name_and_by_position() {
    let source = r#"
        import ambient.json;
        import ambient.{catch, throw};

        fn main(): int => catch(sum) as int else 0;

        fn sum(): int with throw {
            let read = json.parse("{\"ports\": [80, 443], \"name\": \"web\"}");
            let second = json.at(json.get(read, "ports"), 1) as int else 0;
            // Nothing is there, and what is there is no object or array.
            let missing = json.get(read, "host") as int else 1;
            let scalar = json.get(json.get(read, "name"), "x") as int else 2;
            let past = json.at(json.get(read, "ports"), 2) as int else 4;
            return second + missing + scalar + past;
        }
    "#;

    assert_eq!(int(source), 450);
}

/// What is between the quotes of a string as the library writes one,
/// of some of `characters`.
fn inside(characters: &[&'static str]) -> impl Strategy<Value = String> + use<> {
    let character = prop::sample::select(characters.to_vec());
    prop::collection::vec(character, 0..4).prop_map(|parts| parts.concat())
}

/// Characters that are written as themselves, so that names are in
/// the same order written as read.
const PLAIN: [&str; 4] = ["a", "é", "😀", " "];
const ESCAPED: [&str; 4] = ["\\\"", "\\\\", "\\n", "\\u0007"];

/// JSON as the library writes it: nothing between its parts, and the
/// names of an object in order.
fn written() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        any::<bool>().prop_map(|value| value.to_string()),
        any::<i64>().prop_map(|value| value.to_string()),
        any::<i32>().prop_map(|value| format!("{:?}", f64::from(value) / 8.0)),
        inside(&[PLAIN, ESCAPED].concat()).prop_map(|text| format!("\"{text}\"")),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        let members = prop::collection::btree_map(inside(&PLAIN), inner.clone(), 0..3);
        prop_oneof![
            prop::collection::vec(inner, 0..3).prop_map(|items| format!("[{}]", items.join(","))),
            members.prop_map(|members| {
                let each = members
                    .iter()
                    .map(|(name, member)| format!("\"{name}\":{member}"));
                format!("{{{}}}", each.collect::<Vec<_>>().join(","))
            }),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4))]

    #[test]
    fn what_the_library_writes_it_reads_back_the_same(texts in [written(), written(), written(), written()]) {
        let read = rewritten(texts.each_ref().map(String::as_str));

        prop_assert_eq!(read, texts);
    }
}

#[test]
fn a_value_is_told_apart_by_the_type_it_is_asked_for() {
    // No value can be tested for a map, which holds methods; the
    // test made is the nearest that can be, and tells as much here.
    let source = r#"
        import ambient.json;
        import ambient.json.{Json};
        import ambient.map.{Map};
        import ambient.{catch, throw};

        fn kind(value: Json): string {
            match value {
                let members: Map<Json> => { return "object of ${members.size()}"; }
                let items: List<Json> => { return "array of ${items.length()}"; }
                _ => { return "scalar"; }
            }
        }
        fn read(): string with throw {
            let value = json.parse("{\"a\": [1, 2], \"b\": 3}");
            let inner = json.get(value, "a") as List<Json> else [];
            let none = json.get(value, "b") as Map<Json> else null;
            return "${kind(value)}, ${kind(inner)}, ${kind(none)}";
        }
        fn main(): string => catch(read) as string else "threw";
    "#;

    assert_eq!(text(source), "object of 2, array of 2, scalar");
}

#[test]
fn a_cast_that_every_value_passes_tests_nothing() {
    // No value can be tested for JSON, and none need be: what is cast
    // is JSON already, so what the cast falls back to is never reached.
    let source = r"
        import ambient.json.{Json, write};

        fn kept(value: Json): Json => value as Json else null;
        fn main(): string => write(kept([1, 2]));
    ";
    assert_eq!(text(source), "[1,2]");
}
