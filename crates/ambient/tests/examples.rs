//! The packages under `examples/`, read from disk: each still checks.

#![allow(clippy::unwrap_used)]

mod support;

use std::fmt::Write;
use std::path::PathBuf;

use ambient::cli::Command;
use ambient::host::{Request, Response};
use ambient::project::Disk;
use sha2::{Digest, Sha256};
use support::scripted::Scripted;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn every_example_checks() {
    let mut checked = 0;
    for entry in std::fs::read_dir(example("")).unwrap() {
        let path = entry.unwrap().path();
        let command = Command::Check { path: path.clone() };
        let (code, world) = Scripted::default().execute(&Disk, &command);

        assert_eq!((code, world.err.as_str()), (0, ""));
        support::scripted::verify(&Disk, &path);
        checked += 1;
    }

    assert!(checked > 0);
}

#[test]
fn every_test_of_an_example_passes() {
    for entry in std::fs::read_dir(example("")).unwrap() {
        let path = entry.unwrap().path();
        let (code, world) = Scripted::default().execute(&Disk, &Command::Test { path, only: None });

        assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
    }
}

#[test]
fn the_tally_counts_the_words_of_a_file() {
    let world = Scripted {
        files: [("notes".to_owned(), "the cat and the hat\nthe end".into())].into(),
        args: vec!["notes".to_owned()],
        ..Scripted::default()
    };
    let command = Command::Run {
        path: example("tally"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.execute(&Disk, &command);

    assert_eq!((code, world.err.as_str()), (0, ""));
    assert_eq!(world.out, "3 the\n1 and\n1 cat\n1 end\n1 hat\n");
}

#[test]
fn the_stock_keeps_its_counts_in_a_file_of_json() {
    let command = Command::Run {
        path: example("stock"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let world = Scripted {
        files: [("stock.json".to_owned(), r#"{"tea": 2}"#.into())].into(),
        args: ["add", "jam", "3"].map(str::to_owned).to_vec(),
        ..Scripted::default()
    };
    let (code, world) = world.execute(&Disk, &command);

    assert_eq!((code, world.err.as_str()), (0, ""));
    assert_eq!(world.out, "jam         3\ntea         2\n");
    assert_eq!(world.file("stock.json"), r#"{"jam":3,"tea":2}"#);
}

#[test]
fn the_notes_are_served_and_kept_in_a_file_of_json() {
    let request = |method: &str, url: &str, body: &str| ambient::host::Request {
        method: method.to_owned(),
        url: url.to_owned(),
        headers: Vec::new(),
        body: body.into(),
    };
    let world = Scripted {
        files: [("notes.json".to_owned(), "[\"milk\"]".into())].into(),
        requests: [
            request("POST", "/notes", "eggs"),
            request("DELETE", "/notes/0", ""),
            request("GET", "/notes", ""),
        ]
        .into(),
        ..Scripted::default()
    };
    let command = Command::Run {
        path: example("notes"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.execute(&Disk, &command);

    assert_eq!((code, world.err.as_str()), (0, ""));
    let answers: Vec<_> = world
        .answers
        .iter()
        .map(|answer| (answer.status, String::from_utf8_lossy(&answer.body)))
        .collect();
    let expected = [(201, "1"), (204, ""), (200, "[\"eggs\"]")];
    assert_eq!(
        answers,
        expected.map(|(status, body)| (status, body.into()))
    );
    assert_eq!(world.file("notes.json"), "[\"eggs\"]");
}

/// Runs an entry of an example in a world, which must not fail.
fn entry(name: &str, entry: &str, world: Scripted) -> Scripted {
    let command = Command::Run {
        path: example(name),
        entry: entry.to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.execute(&Disk, &command);
    assert_eq!((code, world.err.as_str()), (0, ""));
    world
}

/// What an entry of an example posts to a server that answers `said`.
fn posted(name: &str, start: &str, args: &[&str], url: &str, said: &str) -> Request {
    let response = Response {
        status: 200,
        body: said.into(),
        ..Response::default()
    };
    let sender = Scripted {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        responses: [(format!("POST {url}"), response)].into(),
        ..Scripted::default()
    };
    entry(name, start, sender).sent.remove(0)
}

fn request(method: &str, url: &str, body: &[u8]) -> Request {
    Request {
        method: method.to_owned(),
        url: url.to_owned(),
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

/// The status and body of each answer a server gave.
fn said(world: &Scripted) -> Vec<(i64, String)> {
    let answer = |answer: &Response| {
        let body = String::from_utf8_lossy(&answer.body);
        (answer.status, body.into_owned())
    };
    world.answers.iter().map(answer).collect()
}

#[test]
fn a_server_takes_a_deploy_and_goes_on_serving() {
    let url = "http://127.0.0.1:8080/deploy";
    let sent = posted("hot", "deploy.main", &[], url, "deployed\n");
    let requests = [
        request("GET", "/before", b""),
        request("POST", "/deploy", b"not code"),
        request("POST", "/deploy", &sent.body),
        request("GET", "/after", b""),
    ];
    let server = Scripted {
        requests: requests.into(),
        ..Scripted::default()
    };

    let expected = [
        (200, "hello from /before\n"),
        (400, "not a pack\n"),
        (200, "deployed\n"),
        (200, "hello from /after\n"),
    ];
    let answers = said(&entry("hot", "main.main", server));
    assert_eq!(
        answers,
        expected.map(|(status, body)| (status, body.to_owned()))
    );
}

#[test]
fn a_function_that_was_posted_is_called_by_its_hash() {
    let url = "http://127.0.0.1:8080/";
    let pack = |sample: &str| posted("functions", "publish.main", &[sample], url, "/x").body;
    let (hello, spin) = (pack("hello"), pack("spin"));
    let name = |pack: &[u8]| {
        let hex = |mut hex: String, byte: &u8| {
            let _ = write!(hex, "{byte:02x}");
            hex
        };
        Sha256::digest(pack).iter().fold(String::new(), hex)
    };
    let (greets, spins) = (name(&hello), name(&spin));
    let requests = [
        request("POST", "/", &hello),
        request("POST", "/", &spin),
        request("GET", &format!("/{greets}/ada"), b""),
        request("GET", &format!("/{spins}"), b""),
        request("GET", "/../main", b""),
    ];
    let server = Scripted {
        requests: requests.into(),
        ..Scripted::default()
    };

    let after = entry("functions", "main.main", server);
    let expected = [
        (201, format!("/{greets}\n")),
        (201, format!("/{spins}\n")),
        (200, "hello, ada\n".to_owned()),
        (503, "the function took too long\n".to_owned()),
        (404, "no function is called that\n".to_owned()),
    ];
    assert_eq!(said(&after), expected);
    // Both are kept: what was sent, and what it compiled to.
    assert_eq!(after.files[&format!(".cache/{greets}.pack")], hello);
    assert!(after.files.contains_key(&format!(".cache/{greets}.code")));
}
