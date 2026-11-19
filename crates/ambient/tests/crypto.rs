//! Hashing, HMAC, and random bytes. See "Cryptography" in
//! `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::value::Value;
use support::scripted::Scripted;

/// What `expr`, a string, gives under the command-line target.
fn gives(row: &str, expr: &str) -> String {
    let source = format!(
        "use system.crypto;
        use system.random;
        fn main(): string {row} => {expr};"
    );
    match &Scripted::default().program(&source).0 {
        Ok(Value::String(text)) => text.clone(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn hash(algorithm: &str, data: &str) -> String {
    gives(
        "",
        &format!(r#"crypto.hash(crypto.{algorithm}, "{data}".to_bytes()).hex()"#),
    )
}

#[test]
fn bytes_are_hashed_by_the_algorithm_a_tag_names() {
    // The digests of "abc" each standard gives.
    assert_eq!(
        hash("sha256", "abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hash("sha1", "abc"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert!(hash("sha512", "abc").starts_with("ddaf35a193617abacc417349ae204131"));
    assert_eq!(hash("sha512", "").len(), 128);
}

#[test]
fn an_hmac_is_a_hash_only_the_key_makes() {
    // RFC 4231, test case 2.
    let signed = r#"crypto.hmac(crypto.sha256, "Jefe".to_bytes(), "what do ya want for nothing?".to_bytes())"#;
    assert_eq!(
        gives("", &format!("{signed}.hex()")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[test]
fn digests_are_compared_whole() {
    let same = |left: &str, right: &str| {
        let compared = format!(r#"crypto.same("{left}".to_bytes(), "{right}".to_bytes())"#);
        gives("", &format!("{compared}.to_text()"))
    };
    assert_eq!(same("token", "token"), "true");
    assert_eq!(same("token", "tokem"), "false");
    assert_eq!(same("token", "tok"), "false");
}

#[test]
fn random_bytes_are_drawn_from_the_one_ability() {
    // The scripted world's bits count up from one: seven bytes a draw.
    assert_eq!(
        gives("with random.bits", "random.bytes(9).hex()"),
        "010000000000000200"
    );
    assert_eq!(gives("with random.bits", "random.bytes(0).hex()"), "");
}
