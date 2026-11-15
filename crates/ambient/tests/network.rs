//! HTTP: sending requests and serving them. See "Network" in
//! `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::engine::Engine;
use ambient::host::{self, Request, Response, System, World};
use ambient::store::MemoryStore;
use ambient::value::Value;
use support::scripted::Scripted;
use support::{Outcome, run_entry, value};

/// Runs `main` under the command-line target in `world`, on every
/// backend, and gives what it gave and the world the first left.
fn run(source: &str, world: &Scripted) -> (Outcome, Scripted) {
    let left = RefCell::new(Vec::new());
    let outcome = run_over(source, || {
        let world = Rc::new(RefCell::new(world.clone()));
        left.borrow_mut().push(Rc::clone(&world));
        world
    });
    let first = left.borrow()[0].borrow().clone();
    (outcome, first)
}

/// Runs `main` under the command-line target on every backend, each in
/// a world `make` makes.
fn run_over<W: World + 'static>(source: &str, make: impl Fn() -> Rc<RefCell<W>>) -> Outcome {
    let workspace = support::text::workspace(source);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let target = ambient::system::target();
    let entry = engine.program(&value("main", "main"), &target).unwrap();
    run_entry(&engine, &entry, &[], || {
        let world: Rc<RefCell<dyn World>> = make();
        host::natives(&world)
    })
}

fn text(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn request(method: &str, url: &str, body: &str) -> Request {
    Request {
        method: method.to_owned(),
        url: url.to_owned(),
        headers: Vec::new(),
        body: body.into(),
    }
}

#[test]
fn a_request_is_sent_and_its_response_read() {
    let source = r#"
        import system.http;

        fn main(): string with http.send {
            match http.post("http://example.test/items", "milk".to_bytes()) {
                let error: Error => { return error.message; }
                let response: http.Response => {
                    let kind = http.header(response.headers, "content-type") as string else "none";
                    let ok = http.ok(response) ? "ok" : "not ok";
                    let body = response.body.to_text() as string else "not text";
                    return "${response.status} ${kind} ${body} ${ok}";
                }
            }
        }
    "#;
    let created = Response {
        status: 201,
        headers: vec![("content-type".to_owned(), "text/plain".to_owned())],
        body: "added".into(),
    };
    let world = Scripted {
        responses: [("POST http://example.test/items".to_owned(), created)].into(),
        ..Scripted::default()
    };

    let (outcome, after) = run(source, &world);
    assert_eq!(outcome, Ok(text("201 text/plain added ok")));
    assert_eq!(
        after.sent,
        [request("POST", "http://example.test/items", "milk")]
    );
}

#[test]
fn a_body_that_is_not_text_is_sent_and_read() {
    let source = r#"
        import system.http;
        import ambient.bytes;

        fn main(): string with http.send {
            let sent = bytes.of([0, 255]) as bytes else "".to_bytes();
            match http.post("http://example.test/blob", sent) {
                let error: Error => { return error.message; }
                let response: http.Response => { return response.body.hex(); }
            }
        }
    "#;
    let echoed = Response {
        status: 200,
        headers: Vec::new(),
        body: vec![255, 0, 128],
    };
    let world = Scripted {
        responses: [("POST http://example.test/blob".to_owned(), echoed)].into(),
        ..Scripted::default()
    };

    let (outcome, after) = run(source, &world);
    assert_eq!(outcome, Ok(text("ff0080")));
    assert_eq!(after.sent[0].body, [0, 255]);
}

#[test]
fn a_request_nothing_answers_is_an_error_a_program_matches_on() {
    let source = r#"
        import system.http;

        fn main(): string with http.send {
            match http.get("http://nowhere.test/") {
                let error: Error => {
                    match error.kind {
                        http.unreachable => { return "unreachable"; }
                        _ => { return error.message; }
                    }
                }
                let _: http.Response => { return "answered"; }
            }
        }
    "#;

    assert_eq!(run(source, &Scripted::default()).0, Ok(text("unreachable")));
}

#[test]
fn requests_are_sent_together() {
    let source = r#"
        import system.clock;
        import system.http;

        fn status(url: string): int with http.send {
            match http.get(url) {
                let response: http.Response => { return response.status; }
                let _: Error => { return 0; }
            }
        }

        fn main(): int with http.send {
            let (a, b) = all(fn() => status("http://a.test/"), fn() => status("http://b.test/"));
            return a + b;
        }
    "#;
    let found = |status| Response {
        status,
        ..Response::default()
    };
    let world = Scripted {
        responses: [
            ("GET http://a.test/".to_owned(), found(200)),
            ("GET http://b.test/".to_owned(), found(204)),
        ]
        .into(),
        ..Scripted::default()
    };

    assert_eq!(run(source, &world).0, Ok(Value::Int(404)));
}

/// A server that greets by the path it was asked for, and fails on one.
const SERVER: &str = r#"
    import system.http;

    fn greet(request: http.Request): http.Response with throw {
        if request.url == "/fail" {
            fail(http.failed, "no");
        }
        if request.url == "/fault" {
            return http.text(200, (1 / (request.body.length())).to_text());
        }
        let body = request.body.to_text() as string else "not text";
        return http.text(200, "${request.method} ${request.url} ${body}");
    }
"#;

#[test]
fn a_server_answers_each_request_it_is_sent() {
    let source = format!(
        "{SERVER}
        fn main(): int with http {{
            match http.listen!(\"127.0.0.1\", 8080) {{
                let server: http.Server => {{
                    http.serve(server, 2, greet);
                    return server.port;
                }}
                let _: Error => {{ return 0; }}
            }}
        }}"
    );
    let world = Scripted {
        requests: [
            request("GET", "/a", ""),
            request("POST", "/b", "body"),
            request("GET", "/c", ""),
        ]
        .into(),
        ..Scripted::default()
    };

    let (outcome, after) = run(&source, &world);
    assert_eq!(outcome, Ok(Value::Int(8080)));
    let body = |answer: &Response| String::from_utf8_lossy(&answer.body).into_owned();
    let bodies: Vec<String> = after.answers.iter().map(body).collect();
    assert_eq!(bodies, ["GET /a ", "POST /b body", "GET /c "]);
}

#[test]
fn a_request_the_handler_fails_on_is_answered_and_the_server_goes_on() {
    let source = format!(
        "{SERVER}
        fn main(): null with http {{
            match http.listen!(\"127.0.0.1\", 0) {{
                let server: http.Server => {{ http.serve(server, 1, greet); }}
                let _: Error => {{}}
            }}
        }}"
    );
    let world = Scripted {
        requests: [
            request("GET", "/fail", ""),
            request("GET", "/fault", ""),
            request("GET", "/fine", ""),
        ]
        .into(),
        ..Scripted::default()
    };

    let (_, after) = run(&source, &world);
    let statuses: Vec<i64> = after.answers.iter().map(|answer| answer.status).collect();
    assert_eq!(statuses, [500, 500, 200]);
}

#[test]
fn a_program_serves_and_sends_over_a_real_socket() {
    // One task serves a single request and closes; the other sends it.
    let source = format!(
        "{SERVER}
        fn asked(port: int): string with http.send {{
            match http.post(\"http://127.0.0.1:${{port}}/hello?x=1\", \"there\".to_bytes()) {{
                let response: http.Response => {{
                    let body = response.body.to_text() as string else \"not text\";
                    return \"${{response.status}} ${{body}}\";
                }}
                let error: Error => {{ return error.message; }}
            }}
        }}

        fn once(server: http.Server): fn(http.Request): http.Response with throw with http.close =>
            fn(request: http.Request): http.Response with throw {{
                http.close!(server);
                return greet(request);
            }};

        fn main(): string with http {{
            match http.listen!(\"127.0.0.1\", 0) {{
                let server: http.Server => {{
                    let (_, answer) = all(
                        fn() => http.serve(server, 1, once(server)),
                        fn() => asked(server.port),
                    );
                    return answer;
                }}
                let error: Error => {{ return error.message; }}
            }}
        }}"
    );

    let outcome = run_over(&source, || Rc::new(RefCell::new(System::new(Vec::new()))));
    assert_eq!(outcome, Ok(text("200 POST /hello?x=1 there")));
}
