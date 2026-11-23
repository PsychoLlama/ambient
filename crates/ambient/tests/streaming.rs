//! Responses whose bodies arrive as streams, and whom a request
//! trusts. See "Network" in `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;

use ambient::host::{Response, System, Trust};
use ambient::value::Value;
use support::Outcome;
use support::scripted::{Scripted, over};

fn text(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn real(source: &str) -> Outcome {
    over(source, || Rc::new(RefCell::new(System::new(Vec::new()))))
}

/// What a program here reads a body with.
const SAID: &str = r#"
    use system.http;
    use system.stream;

    fn said(read: bytes | null | Error): string {
        match read {
            null => { return "ended"; }
            let error: Error => { return error.message; }
            let piece: bytes => { return piece.to_text() ?? "not text"; }
        }
    }
"#;

#[test]
fn a_body_is_read_a_piece_at_a_time_from_a_stream() {
    let source = format!(
        r#"{SAID}
        fn main(): string with http.open, stream {{
            match http.stream("http://example.test/events") {{
                let error: Error => {{ return error.message; }}
                let opened: http.Streamed => {{
                    let first = said(stream.read!(opened.body, 3));
                    let rest = said(stream.read_all(opened.body));
                    return "${{opened.status}} ${{first}} ${{rest}}";
                }}
            }}
        }}
    "#
    );
    let events = Response {
        status: 200,
        headers: Vec::new(),
        body: "onetwo".into(),
    };
    let world = Scripted {
        responses: [("GET http://example.test/events".to_owned(), events)].into(),
        ..Scripted::default()
    };

    let (outcome, after) = world.program(&source);

    assert_eq!(outcome, Ok(text("200 one two")));
    assert_eq!(after.trusted, [None]);
}

#[test]
fn a_body_is_read_while_the_server_is_still_sending_it() {
    // The server sends the rest only once the first piece was read,
    // which a body held whole would never be.
    let outcome = real(&format!(
        r#"{SAID}
        use system.net;
        use ambient.tasks;

        fn serve(listener: net.Listener, read: Cell<bool>): null with net.accept, stream {{
            match net.accept!(listener) {{
                null => {{ return null; }}
                let peer: stream.Stream => {{
                    let _ = stream.read!(peer, 4096);
                    let _ = stream.write_text(peer, "HTTP/1.1 200 OK\r\n\r\nfirst");
                    read.wait(false);
                    let _ = stream.write_text(peer, "second");
                    stream.close!(peer);
                    return null;
                }}
            }}
        }}

        fn ask(port: int, read: Cell<bool>): string with http.open, stream {{
            match http.stream("http://127.0.0.1:${{port}}/events") {{
                let error: Error => {{ return error.message; }}
                let opened: http.Streamed => {{
                    let first = said(stream.read!(opened.body, 64));
                    let _ = read.replace(false, true);
                    let rest = said(stream.read_all(opened.body));
                    return "${{first}} then ${{rest}}";
                }}
            }}
        }}

        fn main(): string with net, http.open, stream {{
            match net.listen!("127.0.0.1", 0) {{
                let error: Error => {{ return error.message; }}
                let server: net.Server => {{
                    let read = cell::<bool>(false);
                    let (_, answer) = tasks.all(
                        fn() => serve(server.listener, read),
                        fn() => ask(server.port, read),
                    );
                    return answer;
                }}
            }}
        }}
    "#
    ));

    assert_eq!(outcome, Ok(text("first then second")));
}

/// A server that shows it is `bridge.test`, by a certificate from an
/// authority of its own, and says `secret` to whoever it is secured
/// with. Gives its port, the authority's certificate, and the SHA-256
/// of its own.
fn bridge() -> (u16, String, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use sha2::Digest;

    let mut authority = CertificateParams::new(Vec::new()).unwrap();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let authority = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let shown = CertificateParams::new(vec!["bridge.test".to_owned()]).unwrap();
    let shown = shown.signed_by(&key, &authority).unwrap();
    let digest = sha2::Sha256::digest(shown.der()).to_vec();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![shown.der().clone()], key)
        .unwrap();
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let secured = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
            let mut secured = rustls::StreamOwned::new(secured, connection.unwrap());
            // One that does not trust the server leaves before it asks.
            if secured.read(&mut [0; 4096]).is_err() {
                continue;
            }
            let answer = "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 6\r\n\r\nsecret";
            let _ = secured.write_all(answer.as_bytes());
            secured.conn.send_close_notify();
            let _ = secured.flush();
        }
    });
    (port, authority.pem(), digest)
}

/// `roots` as a program writes them, for [`asked`].
fn by(roots: &str) -> String {
    format!("{roots:?}")
}

/// A certificate's digest as a program writes it, for [`asked`].
fn pinned(digest: &[u8]) -> String {
    let pairs: Vec<String> = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("0x[{}]", pairs.join(" "))
}

/// What asking `url` gives a program that trusts `roots` and `pin`,
/// each as it is written or `null`, and finds every server at this
/// machine.
fn asked(url: &str, roots: &str, pin: &str) -> Outcome {
    real(&format!(
        r#"{SAID}
        fn main(): string with http.open, stream {{
            let request = {{ method: "GET", url: "{url}", headers: [], body: "".to_bytes() }};
            match http.send_trusting(request, {{ roots: {roots}, pin: {pin}, address: "127.0.0.1" }}) {{
                let response: http.Response => {{ return said(response.body); }}
                let error: Error => {{
                    return error.kind == http.untrusted ? "untrusted: ${{error.message}}" : error.message;
                }}
            }}
        }}
    "#
    ))
}

/// Whether [`asked`] was refused as untrusted, for a reason that says
/// `why`.
fn untrusted(outcome: &Outcome, why: &str) -> bool {
    matches!(outcome, Ok(Value::String(said)) if said.starts_with("untrusted: ") && said.contains(why))
}

#[test]
fn a_server_is_trusted_by_the_roots_a_request_names_for_the_name_in_its_url() {
    let (port, roots, _) = bridge();
    let named = format!("https://bridge.test:{port}/");
    let other = format!("https://other.test:{port}/");

    assert_eq!(asked(&named, &by(&roots), "null"), Ok(text("secret")));
    // The world's authorities did not sign it, and it is not `other`.
    assert!(untrusted(&asked(&named, "null", "null"), ""));
    assert!(untrusted(&asked(&other, &by(&roots), "null"), ""));
}

#[test]
fn a_server_is_trusted_by_the_certificate_a_request_pins_whatever_its_name() {
    let (port, roots, digest) = bridge();
    let other = format!("https://other.test:{port}/");
    let at = format!("https://127.0.0.1:{port}/");
    let another: Vec<u8> = digest.iter().map(|byte| !byte).collect();

    assert_eq!(asked(&other, "null", &pinned(&digest)), Ok(text("secret")));
    assert_eq!(asked(&at, "null", &pinned(&digest)), Ok(text("secret")));
    // A person is told it was the pin that refused the server.
    let refused = asked(&other, "null", &pinned(&another));
    assert!(untrusted(&refused, "pinned"), "{refused:?}");
    // A pin asks no authority, so naming some beside it is a mistake.
    let both = asked(&other, &by(&roots), &pinned(&digest));
    assert!(
        matches!(&both, Ok(Value::String(said)) if said.contains("roots")),
        "{both:?}"
    );
}

#[test]
fn whom_a_request_trusts_is_what_the_world_is_told() {
    let source = format!(
        r#"{SAID}
        fn main(): string with http.open, stream {{
            let request = {{ method: "GET", url: "https://bridge.test/", headers: [], body: "".to_bytes() }};
            match http.send_trusting(request, {{ roots: "a root", pin: 0x[01 02], address: null }}) {{
                let response: http.Response => {{ return said(response.body); }}
                let error: Error => {{ return error.message; }}
            }}
        }}
    "#
    );
    let secret = Response {
        status: 200,
        headers: Vec::new(),
        body: "secret".into(),
    };
    let world = Scripted {
        responses: [("GET https://bridge.test/".to_owned(), secret)].into(),
        ..Scripted::default()
    };

    let (outcome, after) = world.program(&source);

    let trusted = Trust {
        roots: Some("a root".to_owned()),
        pin: Some(vec![1, 2]),
        address: None,
    };
    assert_eq!(outcome, Ok(text("secret")));
    assert_eq!(after.trusted, [Some(trusted)]);
}

/// What a program gives that pairs with the server at `url`, found at
/// this machine: it learns the certificate the server shows, pins it,
/// and asks the server again under that pin.
fn paired(url: &str) -> Outcome {
    real(&format!(
        r#"{SAID}
        use system.crypto;

        fn main(): string with http.certificate, http.open, stream, crypto.hash {{
            match http.certificate!("{url}", "127.0.0.1") {{
                let error: Error => {{
                    return error.kind == http.invalid ? "invalid" : error.message;
                }}
                let shown: bytes => {{
                    let pin = crypto.hash!(crypto.sha256, shown);
                    let request = {{ method: "GET", url: "{url}", headers: [], body: "".to_bytes() }};
                    match http.send_trusting(request, {{ roots: null, pin, address: "127.0.0.1" }}) {{
                        let response: http.Response => {{ return said(response.body); }}
                        let error: Error => {{ return error.message; }}
                    }}
                }}
            }}
        }}
    "#
    ))
}

#[test]
fn the_certificate_a_server_shows_is_learned_to_pin_it() {
    let (port, _, _) = bridge();

    // No authority vouches for it, and it is not for `other.test`.
    let other = format!("https://other.test:{port}/");
    assert_eq!(paired(&other), Ok(text("secret")));
    // Only a secured connection shows a certificate.
    let plain = format!("http://bridge.test:{port}/");
    assert_eq!(paired(&plain), Ok(text("invalid")));
}

#[test]
fn the_certificate_a_server_shows_is_what_the_world_says() {
    let source = r#"
        use system.http;

        fn main(): bytes | string with http.certificate {
            match http.certificate!("https://bridge.test/", null) {
                let error: Error => { return error.message; }
                let shown: bytes => { return shown; }
            }
        }
    "#;
    let world = Scripted {
        certificates: [("https://bridge.test/".to_owned(), vec![1, 2])].into(),
        ..Scripted::default()
    };

    assert_eq!(world.program(source).0, Ok(Value::Bytes(vec![1, 2])));
}
