//! A value as bytes and back, for a program that keeps one. See
//! "Values as bytes" in `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::value::Value;
use support::scripted::Scripted;

/// What `main`, with this body, gives under the command-line target.
fn gives(body: &str) -> Value {
    let source = format!(
        "use system.exchange;
        use ambient.Error;
        fn main(): unknown {{ {body} }}"
    );
    Scripted::default().program(&source).0.unwrap()
}

#[test]
fn a_value_read_back_from_its_bytes_is_the_value() {
    let same = gives(
        r#"let kept = { name: "ada", marks: [1, 2.5, null], pair: (true, 0x[00 ff]) };
        match exchange.encode(kept) {
            let error: Error => { return error.message; }
            let encoded: bytes => { return exchange.decode(encoded) == kept; }
        }"#,
    );

    assert_eq!(same, Value::Bool(true));
}

#[test]
fn only_plain_data_has_bytes() {
    let kind = gives(
        "match exchange.encode({ work: fn(n: int): int => n }) {
            let error: Error => { return error.kind == exchange.invalid; }
            _ => { return false; }
        }",
    );

    assert_eq!(kind, Value::Bool(true));
}

#[test]
fn bytes_that_are_no_value_are_refused() {
    let kind = gives(
        "match exchange.decode(0x[ff ff ff]) {
            let error: Error => { return error.kind == exchange.invalid; }
            _ => { return false; }
        }",
    );

    assert_eq!(kind, Value::Bool(true));
}
