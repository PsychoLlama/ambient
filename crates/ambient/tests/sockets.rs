//! Streams and sockets below HTTP. See "Streams and sockets" in
//! `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::host::System;
use ambient::value::Value;
use support::Outcome;
use support::scripted::{Scripted, over};

fn text(text: &str) -> Value {
    Value::String(text.to_owned())
}

/// What every program here reads a stream with: what was read as
/// text, or what there was instead.
const SAID: &str = r#"
    use system.net;
    use system.stream;
    use ambient.tasks;

    fn said(read: bytes | null | Error): string {
        match read {
            null => { return "ended"; }
            let error: Error => { return error.message; }
            let piece: bytes => { return piece.to_text() ?? "not text"; }
        }
    }

    /// Sends back what a connection first says, in capitals, once.
    fn shout(listener: net.Listener): null with net, stream {
        match net.accept!(listener) {
            null => { return null; }
            let peer: stream.Stream => {
                let heard = said(stream.read!(peer, 64));
                let _ = stream.write_text(peer, heard.to_upper());
                stream.close!(peer);
                return null;
            }
        }
    }

    /// What a peer answers `words` with.
    fn ask(connected: stream.Stream | Error, words: string): string with stream {
        match connected {
            let error: Error => { return error.message; }
            let peer: stream.Stream => {
                let _ = stream.write_text(peer, words);
                return said(stream.read_all(peer));
            }
        }
    }
"#;

/// Runs `main` of `SAID` and `source` in the real process.
fn real(source: &str) -> Outcome {
    let source = format!("{SAID}{source}");
    over(&source, || Rc::new(RefCell::new(System::new(Vec::new()))))
}

#[test]
fn a_stream_is_written_to_and_read_a_piece_at_a_time_to_its_end() {
    let source = r#"
        fn main(): string with net, stream {
            match net.connect!("example.test", 7) {
                let error: Error => { return error.message; }
                let peer: stream.Stream => {
                    let _ = stream.write_text(peer, "hi");
                    let first = said(stream.read!(peer, 2));
                    let rest = said(stream.read_all(peer));
                    return "${first} ${rest} ${said(stream.read!(peer, 2))}";
                }
            }
        }
    "#;
    let world = Scripted {
        peers: [("example.test:7".to_owned(), b"hello".to_vec())].into(),
        ..Scripted::default()
    };

    let (outcome, after) = world.program(&format!("{SAID}{source}"));

    assert_eq!(outcome, Ok(text("he llo ended")));
    assert_eq!(after.wrote["example.test:7"], b"hi");
}

#[test]
fn connecting_where_nothing_answers_is_an_error() {
    let source = r#"
        fn main(): string with net, stream => ask(net.connect!("nowhere.test", 7), "hi");
    "#;

    let (outcome, _) = Scripted::default().program(&format!("{SAID}{source}"));

    assert_eq!(outcome, Ok(text("nothing answers at nowhere.test:7")));
}

#[test]
fn a_program_listens_and_connects_over_a_real_socket() {
    let outcome = real(
        r#"
        fn main(): string with net, stream {
            match net.listen!("127.0.0.1", 0) {
                let error: Error => { return error.message; }
                let server: net.Server => {
                    let (_, answer) = tasks.all(
                        fn() => shout(server.listener),
                        fn() => ask(net.connect!("127.0.0.1", server.port), "hello"),
                    );
                    return answer;
                }
            }
        }
    "#,
    );

    assert_eq!(outcome, Ok(text("HELLO")));
}

#[test]
fn a_program_listens_and_connects_over_a_unix_socket() {
    let path = std::env::temp_dir().join(format!("ambient-{}.sock", std::process::id()));
    let path = path.to_str().unwrap();
    let outcome = real(&format!(
        r#"
        fn main(): string with net, stream {{
            match net.listen_unix!("{path}") {{
                let error: Error => {{ return error.message; }}
                let listener: net.Listener => {{
                    let (_, answer) = tasks.all(
                        fn() => shout(listener),
                        fn() => ask(net.connect_unix!("{path}"), "hello"),
                    );
                    net.close!(listener);
                    return answer;
                }}
            }}
        }}
    "#
    ));

    assert_eq!(outcome, Ok(text("HELLO")));
    assert!(!std::path::Path::new(path).exists());
}

#[test]
fn a_datagram_says_where_it_came_from() {
    let outcome = real(
        r#"
        fn heard(from: net.Bound, to: net.Bound): string with net {
            let _ = net.send_to!(from.socket, "ping".to_bytes(), "127.0.0.1", to.port);
            match net.receive!(to.socket) {
                null => { return "closed"; }
                let datagram: net.Datagram => {
                    let came = datagram.port == from.port ? "from it" : "from elsewhere";
                    return "${said(datagram.data)} ${datagram.host} ${came}";
                }
            }
        }

        fn main(): string with net {
            match (net.bind!("127.0.0.1", 0, false), net.bind!("127.0.0.1", 0, false)) {
                (let from: net.Bound, let to: net.Bound) => { return heard(from, to); }
                _ => { return "not bound"; }
            }
        }
    "#,
    );

    assert_eq!(outcome, Ok(text("ping 127.0.0.1 from it")));
}

#[test]
fn what_waits_on_a_listener_or_a_socket_that_is_closed_is_given_nothing() {
    let outcome = real(
        r#"
        fn main(): string with net {
            let listening = net.listen!("127.0.0.1", 0);
            let bound = net.bind!("127.0.0.1", 0, true);
            match (listening, bound) {
                (let server: net.Server, let bound: net.Bound) => {
                    let (accepted, received, _) = tasks.all(
                        fn() => net.accept!(server.listener) == null,
                        fn() => net.receive!(bound.socket) == null,
                        fn() {
                            net.close!(server.listener);
                            net.close!(bound.socket);
                        },
                    );
                    return "${accepted} ${received}";
                }
                _ => { return "not opened"; }
            }
        }
    "#,
    );

    assert_eq!(outcome, Ok(text("true true")));
}
