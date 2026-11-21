//! The packages under `examples/`, read from disk: each still checks.

#![allow(clippy::unwrap_used)]

mod support;

use std::fmt::Write;
use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
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
        let command = Command::Check {
            at: Chosen::default(),
        };
        let (code, world) = Scripted::within(&path).execute(&Disk, &command);

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
        let test = Command::Test {
            at: Chosen::default(),
            only: None,
        };
        let (code, world) = Scripted::within(path).execute(&Disk, &test);

        assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
    }
}

#[test]
fn the_logging_package_passes_its_tests() {
    let path = example("../packages/logging");
    let test = Command::Test {
        at: Chosen::default(),
        only: None,
    };
    let (code, world) = Scripted::within(path).execute(&Disk, &test);

    assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
}

#[test]
fn the_logs_are_written_for_a_terminal_but_for_what_is_too_detailed() {
    let world = binary_saying("logs", "main", Scripted::default());
    let said: Vec<&str> = world.err.lines().collect();

    // Dim, the time, and plain again; the level in its color; then
    // the message, its context, and where it was said.
    let first = "\u{1b}[0m\u{1b}[2m[00:00:00.000]\u{1b}[0m \u{1b}[0m\u{1b}[32mINFO\u{1b}[0m  \
                 listening {\"port\":8080} \u{1b}[0m\u{1b}[2mshop\u{1b}[0m";
    assert_eq!(said[0], first);
    assert_eq!(said.len(), 6);
    assert!(!world.err.contains("query"));
}

#[test]
fn the_tally_counts_the_words_of_a_file() {
    let world = Scripted {
        files: [("notes".to_owned(), "the cat and the hat\nthe end".into())].into(),
        args: vec!["notes".to_owned()],
        ..Scripted::default()
    };
    let command = Command::Run {
        at: Chosen::default(),
        bin: "main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.inside(example("tally")).execute(&Disk, &command);

    assert_eq!((code, world.err.as_str()), (0, ""));
    assert_eq!(world.out, "3 the\n1 and\n1 cat\n1 end\n1 hat\n");
}

#[test]
fn the_stock_keeps_its_counts_in_a_file_of_json() {
    let command = Command::Run {
        at: Chosen::default(),
        bin: "main".to_owned(),
        args: Vec::new(),
    };
    let world = Scripted {
        files: [("stock.json".to_owned(), r#"{"tea": 2}"#.into())].into(),
        args: ["add", "jam", "3"].map(str::to_owned).to_vec(),
        ..Scripted::default()
    };
    let (code, world) = world.inside(example("stock")).execute(&Disk, &command);

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
        at: Chosen::default(),
        bin: "main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.inside(example("notes")).execute(&Disk, &command);

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

/// Runs a binary of an example in a world. It must end well, whatever
/// it wrote to standard error.
fn binary_saying(name: &str, bin: &str, world: Scripted) -> Scripted {
    let command = Command::Run {
        at: Chosen::default(),
        bin: bin.to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.inside(example(name)).execute(&Disk, &command);
    assert_eq!(code, 0, "{}", world.err);
    world
}

/// Runs a binary of an example in a world, which must not fail.
fn binary(name: &str, bin: &str, world: Scripted) -> Scripted {
    let command = Command::Run {
        at: Chosen::default(),
        bin: bin.to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.inside(example(name)).execute(&Disk, &command);
    assert_eq!((code, world.err.as_str()), (0, ""));
    world
}

/// What a binary of an example posts to a server that answers `said`.
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
    binary(name, start, sender).sent.remove(0)
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
    let sent = posted("hot", "deploy", &[], url, "deployed\n");
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
    let answers = said(&binary("hot", "main", server));
    assert_eq!(
        answers,
        expected.map(|(status, body)| (status, body.to_owned()))
    );
}

#[test]
fn a_function_that_was_posted_is_called_by_its_hash() {
    let url = "http://127.0.0.1:8080/";
    let pack = |sample: &str| posted("functions", "publish", &[sample], url, "/x").body;
    let (hello, sum) = (pack("hello"), pack("sum"));
    let name = |pack: &[u8]| {
        let hex = |mut hex: String, byte: &u8| {
            let _ = write!(hex, "{byte:02x}");
            hex
        };
        Sha256::digest(pack).iter().fold(String::new(), hex)
    };
    let (greets, sums) = (name(&hello), name(&sum));
    let requests = [
        request("POST", "/", &hello),
        request("POST", "/", &sum),
        request("GET", &format!("/{greets}/ada"), b""),
        request("POST", &format!("/{sums}"), b"[1, 2, 3]"),
        request("GET", "/../main", b""),
    ];
    let server = Scripted {
        requests: requests.into(),
        ..Scripted::default()
    };

    let after = binary("functions", "main", server);
    let expected = [
        (201, format!("/{greets}\n")),
        (201, format!("/{sums}\n")),
        (200, "hello, ada\n".to_owned()),
        (200, "6".to_owned()),
        (404, "no function is called that\n".to_owned()),
    ];
    assert_eq!(said(&after), expected);
    // Both are kept: what was sent, and what it compiled to.
    assert_eq!(after.files[&format!(".cache/{greets}.pack")], hello);
    assert!(after.files.contains_key(&format!(".cache/{greets}.code")));
}

#[test]
fn the_worlds_play_a_game_back_from_what_was_recorded() {
    let play = |world: Scripted| {
        let command = Command::Run {
            at: Chosen::default(),
            bin: "main".to_owned(),
            args: Vec::new(),
        };
        let world = Scripted {
            args: vec!["game.tape".to_owned()],
            ..world
        };
        let (code, after) = world.inside(example("worlds")).execute(&Disk, &command);
        assert_eq!((code, after.err.as_str()), (0, ""));
        after
    };

    let recorded = play(Scripted::default());
    let total = recorded
        .out
        .lines()
        .last()
        .unwrap()
        .replace("recorded", "played back");
    assert_eq!(recorded.rolled, 2);

    // The game is played again from the file alone: no die is rolled,
    // and what the game said is not said again.
    let files = recorded.files;
    let replayed = play(Scripted {
        files,
        ..Scripted::default()
    });
    assert_eq!(replayed.out, format!("{total}\n"));
    assert_eq!(replayed.rolled, 0);
}
