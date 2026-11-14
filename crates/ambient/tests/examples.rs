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
        let (code, world) = Scripted::default().execute(&Disk, &Command::Test { path });

        assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
    }
}

#[test]
fn the_tally_counts_the_words_of_a_file() {
    let world = Scripted {
        files: [(
            "notes".to_owned(),
            "the cat and the hat\nthe end".to_owned(),
        )]
        .into(),
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
        files: [("stock.json".to_owned(), r#"{"tea": 2}"#.to_owned())].into(),
        args: ["add", "jam", "3"].map(str::to_owned).to_vec(),
        ..Scripted::default()
    };
    let (code, world) = world.execute(&Disk, &command);

    assert_eq!((code, world.err.as_str()), (0, ""));
    assert_eq!(world.out, "jam         3\ntea         2\n");
    assert_eq!(world.files["stock.json"], r#"{"jam":3,"tea":2}"#);
}
