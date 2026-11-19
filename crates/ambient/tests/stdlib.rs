//! The methods the standard library gives each shape, which are source
//! like any program. See `spec/methods.md` and `spec/iteration.md`.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::{evaluates, int};

/// What an expression that gives a list of strings gives, joined by `|`.
fn pieces(expr: &str) -> String {
    evaluates(&format!("({expr}).join_with(\"|\")"))
}

// Strings

#[test]
fn a_string_is_searched() {
    assert_eq!(
        int(r#"fn main(): int => "a needle".find("need") as int else 0 - 1;"#),
        2
    );
    assert_eq!(
        int(r#"fn main(): int => "héllo".find("llo") as int else 0 - 1;"#),
        3
    );
    assert_eq!(
        evaluates(
            r#"match ("abc".contains("bc"), "abc".contains("cb")) { (true, false) => "yes", _ => "no" }"#
        ),
        "yes"
    );
    assert_eq!(
        evaluates(
            r#"match ("abc".starts_with("ab"), "abc".ends_with("bc"), "abc".starts_with("abcd")) { (true, true, false) => "yes", _ => "no" }"#
        ),
        "yes"
    );
}

#[test]
fn a_string_is_split_where_its_separator_is() {
    assert_eq!(pieces(r#""a, b,, c".split(", ")"#), "a|b,|c");
    assert_eq!(pieces(r#""abc".split("")"#), "abc");
    assert_eq!(pieces(r#"",a,".split(",")"#), "|a|");
}

#[test]
fn lines_leave_out_their_endings() {
    assert_eq!(pieces(r#""one\r\ntwo\n\nfour\n".lines()"#), "one|two||four");
    assert_eq!(pieces(r#""".lines()"#), "");
}

#[test]
fn a_string_is_trimmed_and_repeated() {
    assert_eq!(evaluates(r#""  \t padded\n ".trim()"#), "padded");
    assert_eq!(evaluates(r#""   ".trim() ++ "ab".repeat(3)"#), "ababab");
}

// Lists

#[test]
fn a_list_is_mapped_filtered_and_folded() {
    let source = "
        fn main(): int => [1, 2, 3, 4]
            .map(fn(n) => n * 10)
            .filter(fn(n) => n > 10)
            .fold(0, fn(total, n) => total + n);
    ";

    assert_eq!(int(source), 90);
}

#[test]
fn a_list_is_sorted_by_what_comes_before_what() {
    let sorted = r#"["pear", "fig", "apple", "kiwi"].sort_by(fn(a, b) => a < b)"#;
    assert_eq!(pieces(sorted), "apple|fig|kiwi|pear");

    // Members neither before the other keep their order.
    let stable = r#"["bb", "a", "cc", "d"].sort_by(fn(a, b) => a.length() < b.length())"#;
    assert_eq!(pieces(stable), "a|d|bb|cc");
}

#[test]
fn a_list_is_reversed_and_summed() {
    assert_eq!(pieces(r#"["a", "b", "c"].reverse()"#), "c|b|a");
    assert_eq!(
        int("fn main(): int => [1, 2, 3].sum() + ([4].first() as int else 0);"),
        10
    );
}

// Iterators

#[test]
fn an_iterator_is_walked_for_what_a_function_does() {
    let source = "
        fn main(): int {
            let seen = cell<int>(0);
            range(0, 5).each(fn(n) => seen.update(fn(total) => total + n));
            return seen.read();
        }
        use ambient.iterator.range;
    ";

    assert_eq!(int(source), 10);
}

#[test]
fn iterators_are_numbered_chained_and_skipped() {
    let numbered = r#"["a", "b"].iter().enumerate().map(fn(pair) => match pair { (let n, let item) => n.to_text() ++ item }).to_list()"#;
    assert_eq!(pieces(numbered), "0a|1b");

    let chained = r#"["a", "b", "c"].iter().skip(1).chain(["z"].iter()).to_list()"#;
    assert_eq!(pieces(chained), "b|c|z");
}

#[test]
fn an_iterator_is_asked_about_its_items() {
    let source = "
        use ambient.iterator.range;

        fn main(): int {
            let found = range(1, 100).find(fn(n) => n * n > 50) as int else 0;
            let any = match range(0, 3).some(fn(n) => n == 2) { true => 10, false => 0 };
            let each = match range(0, 3).every(fn(n) => n < 2) { true => 100, false => 0 };
            return found + any + each + range(0, 7).count() * 1000;
        }
    ";

    assert_eq!(int(source), 8 + 10 + 7000);
}

#[test]
fn a_character_and_its_scalar_value_convert_both_ways() {
    let code = |text: &str| evaluates(&format!("({text}.code() as int else -1).to_text()"));
    let character =
        |code: &str| evaluates(&format!(r#"{code}.to_character() as string else "none""#));

    assert_eq!(code(r#""a""#), "97");
    assert_eq!(code(r#""é""#), "233");
    // Not one character: none, and two.
    assert_eq!(code(r#""""#), "-1");
    assert_eq!(code(r#""ab""#), "-1");
    assert_eq!(character("233"), "é");
    assert_eq!(character("128512"), "😀");
    // A surrogate and a number past the last scalar value are no character.
    assert_eq!(character("55296"), "none");
    assert_eq!(character("1114112"), "none");
    assert_eq!(character("(0 - 1)"), "none");
}

#[test]
fn a_string_is_replaced_in_and_padded() {
    assert_eq!(evaluates(r#""a-b-c".replace("-", ", ")"#), "a, b, c");
    assert_eq!(evaluates(r#""abc".replace("", "x")"#), "abc");
    assert_eq!(
        evaluates(r#""7".pad_start(3, "0") ++ "|" ++ "é".pad_end(3, ".")"#),
        "007|é.."
    );
    assert_eq!(
        evaluates(r#""wide".pad_start(2, " ") ++ "héllo".count().to_text()"#),
        "wide5"
    );
}

#[test]
fn a_float_is_rounded_to_a_whole_number() {
    let rounded = |float: &str| {
        evaluates(&format!(
            "[{float}.floor(), {float}.ceil(), {float}.round(), {float}.abs()].map(fn(n) => n.to_text()).join_with(\" \")"
        ))
    };

    assert_eq!(rounded("2.5"), "2.0 3.0 3.0 2.5");
    assert_eq!(rounded("(-2.5)"), "-3.0 -2.0 -3.0 2.5");
    assert_eq!(rounded("4.0"), "4.0 4.0 4.0 4.0");
    // Too large to have a fraction.
    assert_eq!(rounded("1e300"), "1e300 1e300 1e300 1e300");
    assert_eq!(
        evaluates("1.5.min(2.5).to_text() ++ 1.5.max(2.5).to_text()"),
        "1.52.5"
    );
}

#[test]
fn a_list_is_asked_about_its_members() {
    let source = r#"
        fn main(): int {
            let words = ["a", "bb", "ccc"];
            let has = words.contains("bb") && !words.contains("z") ? 1 : 0;
            let found = (words.find(fn(word) => word.length() > 1) as string else "").length() * 10;
            let asked = words.some(fn(word) => word == "a") && !words.every(fn(word) => word == "a") ? 100 : 0;
            return has + found + asked + words.take(2).length() * 1000 + words.skip(2).length() * 10000;
        }
    "#;

    assert_eq!(int(source), 12121);
}

#[test]
fn a_string_changes_the_case_of_plain_letters_only() {
    assert_eq!(
        evaluates(r#""Héllo, World 42".to_upper() ++ " " ++ "ÀBC def".to_lower()"#),
        "HéLLO, WORLD 42 Àbc def"
    );
}

#[test]
fn a_boolean_is_written_as_text() {
    assert_eq!(
        evaluates(r#""${1 < 2} and ${(2 < 1).to_text()}""#),
        "true and false"
    );
}
