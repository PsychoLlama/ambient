//! The packages under `examples/`, read from disk: each still checks.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::Command;
use ambient::project::Disk;
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
        let (code, world) = Scripted::default().execute(&Disk, &Command::Check { path });

        assert_eq!((code, world.err.as_str()), (0, ""));
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

/// Runs an entry of the `hot` example in a world, which must not fail.
fn hot(entry: &str, world: Scripted) -> Scripted {
    let command = Command::Run {
        path: example("hot"),
        entry: entry.to_owned(),
        args: Vec::new(),
    };
    let (code, world) = world.execute(&Disk, &command);
    assert_eq!((code, world.err.as_str()), (0, ""));
    world
}

#[test]
fn a_server_takes_a_signed_deploy_and_goes_on_serving() {
    let key = [("hot.key".to_owned(), "secret".into())];
    let deployed = ambient::host::Response {
        status: 200,
        ..ambient::host::Response::default()
    };
    let sender = Scripted {
        files: key.clone().into(),
        responses: [("POST http://127.0.0.1:8080/deploy".to_owned(), deployed)].into(),
        ..Scripted::default()
    };
    // What the deploy sent is what the server is sent.
    let sent = hot("deploy.main", sender).sent.remove(0);
    let request = |url: &str, headers: &[(String, String)]| ambient::host::Request {
        method: "POST".to_owned(),
        url: url.to_owned(),
        headers: headers.to_vec(),
        body: sent.body.clone(),
    };
    let server = Scripted {
        files: key.into(),
        requests: [
            request("/before", &[]),
            request("/deploy", &[]),
            request("/deploy", &sent.headers),
            request("/after", &[]),
        ]
        .into(),
        ..Scripted::default()
    };

    let answers = hot("main.main", server).answers;
    let said: Vec<_> = answers
        .iter()
        .map(|answer| (answer.status, String::from_utf8_lossy(&answer.body)))
        .collect();
    let expected = [
        (200, "hello from /before\n"),
        (403, "not signed with the key\n"),
        (200, "deployed\n"),
        (200, "hello from /after\n"),
    ];
    assert_eq!(said, expected.map(|(status, body)| (status, body.into())));
}
