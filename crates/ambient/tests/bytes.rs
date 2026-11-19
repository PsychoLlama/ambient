//! Bytes: a run of bytes that need not be text. See "Bytes" in
//! `spec/operations.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::exchange::{decode, encode};
use ambient::value::Value;
use proptest::prelude::*;
use support::text::{evaluates, run, text};

#[test]
fn a_string_is_its_encoding_and_back() {
    assert_eq!(evaluates(r#""héllo".to_bytes().length().to_text()"#), "6");
    assert_eq!(evaluates(r#""héllo".to_bytes().hex()"#), "68c3a96c6c6f");
    assert_eq!(
        evaluates(r#""héllo".to_bytes().to_text() ?? "none""#),
        "héllo"
    );
}

#[test]
fn bytes_that_encode_no_string_miss() {
    // The first byte of a two-byte character, alone.
    let cut = r#""é".to_bytes().slice(0, 1)"#;
    assert_eq!(
        evaluates(&format!(r#"(cast::<bytes>({cut}) ?? "".to_bytes()).hex()"#)),
        "c3"
    );
    assert_eq!(
        evaluates(&format!(
            r#"(cast::<bytes>({cut}) ?? "".to_bytes()).to_text() ?? "none""#
        )),
        "none"
    );
}

#[test]
fn bytes_are_read_joined_and_sliced_as_a_list_is() {
    let made = r#"(bytes.of([1, 2, 255]) ?? "".to_bytes())"#;
    let source = |expr: &str| format!("use ambient.bytes;\nfn main(): string => {expr};");
    let gives = |expr: &str| run(&source(expr)).unwrap();
    let text = |text: &str| Value::String(text.to_owned());
    assert_eq!(
        gives(&format!("{made}.join({made}).hex()")),
        text("0102ff0102ff")
    );
    assert_eq!(
        gives(&format!(
            "{made}.to_list().map(fn(b: int): string => b.to_text()).join_with(\" \")"
        )),
        text("1 2 255")
    );
    assert_eq!(
        gives(&format!(
            "({made}.get(2) ?? -1).to_text() + ({made}.get(3) ?? -1).to_text()"
        )),
        text("255-1")
    );
    assert_eq!(
        gives(&format!(
            "({made}.slice(2, 1) ?? \"gone\".to_bytes()).to_text() ?? \"\""
        )),
        text("gone")
    );
}

#[test]
fn a_list_with_a_member_that_is_no_byte_is_no_bytes() {
    let source =
        |list: &str| format!("use ambient.bytes;\nfn main(): bool => bytes.of({list}) == null;");
    assert_eq!(run(&source("[0, 256]")), Ok(Value::Bool(true)));
    assert_eq!(run(&source("[0, -1]")), Ok(Value::Bool(true)));
    assert_eq!(run(&source("[0, 255]")), Ok(Value::Bool(false)));
}

#[test]
fn bytes_are_equal_by_what_they_hold_and_are_no_string() {
    let source = r#"
        fn is_bytes(held: unknown): bool => match held { let _: bytes => true, _ => false };

        fn main(): (bool, bool, bool) {
            let same = "ab".to_bytes() == "a".to_bytes().join("b".to_bytes());
            return (same, is_bytes("ab"), is_bytes("ab".to_bytes()));
        }
    "#;
    let gave = vec![Value::Bool(true), Value::Bool(false), Value::Bool(true)];
    assert_eq!(run(source), Ok(Value::Tuple(gave)));
}

#[test]
fn a_program_gives_its_host_bytes() {
    let source = r#"fn main(): bytes => "hi".to_bytes();"#;
    assert_eq!(run(source), Ok(Value::Bytes(b"hi".to_vec())));
}

proptest! {
    #[test]
    fn bytes_are_encoded_and_read_back(bytes: Vec<u8>) {
        let value = Value::Bytes(bytes);
        prop_assert_eq!(decode(&encode(&value).unwrap()), Ok(value));
    }
}

// Written out

#[test]
fn bytes_are_written_out_in_hexadecimal() {
    let source = "fn main(): bytes => 0x[89 504E\n 47];";
    assert_eq!(run(source), Ok(Value::Bytes(vec![0x89, 0x50, 0x4E, 0x47])));
    assert_eq!(
        run("fn main(): bytes => 0x[];"),
        Ok(Value::Bytes(Vec::new()))
    );
}

#[test]
fn bytes_written_out_are_a_pattern_and_a_type() {
    let source = r#"
        type Png = 0x[89 50];

        fn kind(start: bytes): string {
            match start {
                0x[89 50] => { return "png"; }
                0x[ff d8] => { return "jpeg"; }
                _ => { return "unknown"; }
            }
        }
        fn png(value: unknown): bool => (cast::<Png>(value) ?? null) != null;

        fn main(): string {
            let found = [kind(0x[89 50]), kind(0x[FF D8]), kind("P".to_bytes())].join_with(" ");
            return "${found} ${png(0x[89 50])} ${png(0x[89])} ${png("P")}";
        }
    "#;
    assert_eq!(text(source), "png jpeg unknown true false false");
}

#[test]
fn half_a_byte_is_not_bytes() {
    for source in ["0x[8]", "0x[89 5 0]", "0x[zz]", "0x[89"] {
        let source = format!("fn main(): bytes => {source};");
        assert!(ambient::syntax::read(&source).is_err(), "{source}");
    }
}

#[test]
fn a_comptime_call_gives_bytes() {
    let source = r#"
        fn magic(): bytes => "PNG".to_bytes();
        fn main(): bytes => comptime magic();
    "#;
    assert_eq!(run(source), Ok(Value::Bytes(b"PNG".to_vec())));
}
