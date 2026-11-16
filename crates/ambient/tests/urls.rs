//! URLs: reading and writing them, and what a query holds.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::text;

/// What `expr`, a string, gives with `ambient.url` imported.
fn url(expr: &str) -> String {
    text(&format!(
        "import ambient.url;\nfn main(): string => {expr};"
    ))
}

/// The parts of a URL read from `text`, in one line.
fn parts(source: &str) -> String {
    text(&format!(
        r#"import ambient.url;
        fn pair(pair: (string, string)): string {{
            let (name, value) = pair;
            return "${{name}}={{value}}".replace("{{value}}", value);
        }}
        fn main(): string {{
            match url.parse("{source}") {{
                null => {{ return "not a URL"; }}
                let read: url.Url => {{
                    let port = match read.port {{ null => "-", let port: int => port.to_text() }};
                    let query = read.query.map(pair).join_with("&");
                    let fragment = read.fragment as string else "-";
                    return [read.scheme, read.host, port, read.path, query, fragment].join_with(" | ");
                }}
            }}
        }}"#
    ))
}

#[test]
fn a_url_is_read_into_its_parts() {
    assert_eq!(
        parts("https://example.com:8443/a/b?x=1&y=two+words#top"),
        "https | example.com | 8443 | /a/b | x=1&y=two words | top"
    );
    assert_eq!(parts("http://[::1]:80"), "http | [::1] | 80 | / |  | -");
}

#[test]
fn what_a_server_is_asked_for_is_a_url_with_no_host() {
    assert_eq!(
        parts("/search?q=caf%C3%A9&page=2"),
        " |  | - | /search | q=café&page=2 | -"
    );
}

#[test]
fn a_url_that_cannot_be_read_is_null() {
    assert_eq!(parts("http://example.com:eighty/"), "not a URL");
    assert_eq!(parts("/x?bad=%zz"), "not a URL");
    assert_eq!(parts("/x?bad=%ff"), "not a URL");
}

#[test]
fn a_url_is_written_as_it_was_read() {
    let written = |source: &str| {
        url(&format!(
            "match url.parse(\"{source}\") {{ null => \"\", let read: url.Url => url.write(read) }}"
        ))
    };

    for source in [
        "https://example.com:8443/a/b?x=1&y=two%20words#top",
        "/only/a/path",
    ] {
        assert_eq!(written(source), source);
    }
}

#[test]
fn text_is_encoded_and_read_back() {
    let round = "url.decode(url.encode(\"a b/c?d=é&🙂\")) as string else \"lost\"";

    assert_eq!(url("url.encode(\"a b/c?d=é\")"), "a%20b%2Fc%3Fd%3D%C3%A9");
    assert_eq!(url(round), "a b/c?d=é&🙂");
}

/// Where `reference` leads from `http://a/b/c/d?q=1`, as text.
fn from_base(reference: &str) -> String {
    text(&format!(
        r#"import ambient.url;
        fn main(): string {{
            match (url.parse("http://a/b/c/d?q=1"), url.parse("{reference}")) {{
                (let base: url.Url, let reference: url.Url) => {{
                    return url.write(url.resolve(base, reference));
                }}
                _ => {{ return "not a URL"; }}
            }}
        }}"#
    ))
}

#[test]
fn a_url_is_resolved_against_another_as_a_link_is() {
    // The examples of RFC 3986, section 5.4.
    let led = [
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("?y=2", "http://a/b/c/d?y=2"),
        ("g?y=2", "http://a/b/c/g?y=2"),
        ("#s", "http://a/b/c/d?q=1#s"),
        ("", "http://a/b/c/d?q=1"),
        (".", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../../g", "http://a/g"),
        ("g/../h", "http://a/b/c/h"),
        ("https://b/x/../y", "https://b/y"),
    ];
    for (reference, expected) in led {
        assert_eq!(from_base(reference), expected, "{reference:?}");
    }
}
