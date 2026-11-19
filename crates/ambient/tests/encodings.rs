//! Bytes as hexadecimal and Base64. See `docs/spec/encodings.md`.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::text;

/// What `expr`, a string, gives with the encodings imported.
fn gives(expr: &str) -> String {
    text(&format!(
        "use ambient.base64;\nuse ambient.bytes;\nfn main(): string => {expr};"
    ))
}

/// The Base64 of some text, in the standard form.
fn encoded(plain: &str) -> String {
    gives(&format!(r#"base64.encode("{plain}".to_bytes())"#))
}

/// The text some Base64 says, or what went wrong.
fn decoded(written: &str) -> String {
    gives(&format!(
        r#"match base64.decode("{written}") {{
            null => "not base64",
            let read: bytes => read.to_text() ?? "not text",
        }}"#
    ))
}

#[test]
fn base64_pads_what_three_bytes_do_not_fill() {
    // The vectors of RFC 4648.
    let pairs = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];
    for (plain, written) in pairs {
        assert_eq!(encoded(plain), written);
        assert_eq!(decoded(written), plain);
    }
}

#[test]
fn the_url_form_differs_in_two_characters_and_pads_nothing() {
    let data = "(bytes.of([251, 255, 254]) ?? \"\".to_bytes())";
    assert_eq!(gives(&format!("base64.encode({data})")), "+//+");
    assert_eq!(gives(&format!("base64.encode_url({data})")), "-__-");
    assert_eq!(gives(r#"base64.encode_url("fo".to_bytes())"#), "Zm8");
}

#[test]
fn base64_is_read_in_either_form_padded_or_not() {
    assert_eq!(decoded("Zm8"), "fo");
    let hex = |written: &str| {
        gives(&format!(
            r#"(base64.decode("{written}") ?? "".to_bytes()).hex()"#
        ))
    };
    assert_eq!(
        (hex("+//+"), hex("-__-")),
        ("fbfffe".into(), "fbfffe".into())
    );
}

#[test]
fn text_that_is_not_base64_is_none() {
    assert_eq!(decoded("Zm9v!"), "not base64");
    assert_eq!(decoded("Zm9vY"), "not base64");
    // `Zm8` is `fo`; `Zm9` says the same bytes and two bits more.
    assert_eq!(decoded("Zm9"), "not base64");
    assert_eq!(decoded("Zh=="), "not base64");
}

#[test]
fn hexadecimal_is_read_in_either_case_and_written_in_lower() {
    let read = |written: &str| {
        gives(&format!(
            r#"match bytes.from_hex("{written}") {{ null => "not hex", let read: bytes => read.hex() }}"#
        ))
    };
    assert_eq!(read("00fF10Ab"), "00ff10ab");
    assert_eq!(
        (read("abc"), read("zz")),
        ("not hex".into(), "not hex".into())
    );
}

#[test]
fn every_byte_is_written_and_read_back() {
    let source = r#"
        use ambient.base64;
        use ambient.bytes;
        use ambient.iterator.{range};

        fn main(): bool {
            let all = bytes.of(range(0, 256).to_list()) ?? "".to_bytes();
            let same = fn(read: bytes | null): bool => read == all;
            return same(base64.decode(base64.encode(all)))
                && same(base64.decode(base64.encode_url(all)))
                && same(bytes.from_hex(all.hex()));
        }
    "#;
    assert_eq!(
        support::text::run(source),
        Ok(ambient::value::Value::Bool(true))
    );
}
