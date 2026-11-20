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
/// with. Gives its port and the authority's certificate.
fn bridge() -> (u16, String) {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    let mut authority = CertificateParams::new(Vec::new()).unwrap();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let authority = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let shown = CertificateParams::new(vec!["bridge.test".to_owned()]).unwrap();
    let shown = shown.signed_by(&key, &authority).unwrap();
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
    (port, authority.pem())
}

/// What asking `url` gives a program that trusts `roots`, or whom the
/// world does, and finds every server at this machine.
fn asked(url: &str, roots: Option<&str>) -> Outcome {
    let roots = roots.map_or("null".to_owned(), |roots| format!("{roots:?}"));
    real(&format!(
        r#"{SAID}
        fn main(): string with http.open, stream {{
            let request = {{ method: "GET", url: "{url}", headers: [], body: "".to_bytes() }};
            match http.send_trusting(request, {{ roots: {roots}, address: "127.0.0.1" }}) {{
                let response: http.Response => {{ return said(response.body); }}
                let error: Error => {{
                    return error.kind == http.untrusted ? "untrusted" : error.message;
                }}
            }}
        }}
    "#
    ))
}

#[test]
fn a_server_is_trusted_by_the_roots_a_request_names_for_the_name_in_its_url() {
    let (port, roots) = bridge();
    let named = format!("https://bridge.test:{port}/");
    let other = format!("https://other.test:{port}/");

    assert_eq!(asked(&named, Some(&roots)), Ok(text("secret")));
    // The world's authorities did not sign it, and it is not `other`.
    assert_eq!(asked(&named, None), Ok(text("untrusted")));
    assert_eq!(asked(&other, Some(&roots)), Ok(text("untrusted")));
}

#[test]
fn whom_a_request_trusts_is_what_the_world_is_told() {
    let source = format!(
        r#"{SAID}
        fn main(): string with http.open, stream {{
            let request = {{ method: "GET", url: "https://bridge.test/", headers: [], body: "".to_bytes() }};
            match http.send_trusting(request, {{ roots: "a root", address: null }}) {{
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
        address: None,
    };
    assert_eq!(outcome, Ok(text("secret")));
    assert_eq!(after.trusted, [Some(trusted)]);
}
