//! The JavaScript host (`js/host.js`) runs a program built for it, under
//! Node. See "JavaScript hosts" in `spec/targets.md`.
//!
//! The tests themselves are JavaScript (`js/*.test.js`), since what they test
//! is. This builds the program they run and runs them, so that the one
//! command that runs every test runs these too.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::host::System;
use ambient::store::MemoryStore;

/// The package the tests run: its ID, and its one module. Its target
/// binds an ability that answers at once and one that answers later.
const PACKAGE: &str = "app";
const PROGRAM: &str = r#"
    import ambient.iterator;

    ability say(text: string): null;
    ability sleep(milliseconds: int): null;

    extern handler said for say;
    extern suspending handler slept for sleep;

    target page { say: said, sleep: slept }

    fn main(): int with say, sleep {
        let (counted, waited) = all(fn() => count(3), fn() => linger());
        return counted + waited;
    }

    fn count(to: int): int with say, sleep {
        for number in iterator.range(1, to + 1) {
            say!("counted ${number}");
            sleep!(10);
        }
        return to;
    }

    fn linger(): int with say, sleep {
        sleep!(25);
        say!("still here");
        return 1;
    }
"#;

/// Builds `main` of [`PROGRAM`] under its target as a module at `output`.
fn build(output: &Path) {
    let file = |(path, text): (&str, String)| (PathBuf::from(path), text);
    let manifest = format!("name = \"app\"\nid = \"{PACKAGE}\"");
    let files: BTreeMap<PathBuf, String> = [
        ("app/ambient.toml", manifest),
        ("app/src/main.ab", PROGRAM.to_owned()),
    ]
    .map(file)
    .into();
    let session = Session {
        files: &files,
        store: &MemoryStore::new(),
        world: Rc::new(RefCell::new(System::new(Vec::new()))),
        launcher: None,
    };
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(output.to_owned()),
        module: Some("main.page".to_owned()),
    };
    assert_eq!(session.execute(&command), 0);
}

#[test]
fn the_host_runs_a_program_built_for_it() {
    let module = std::env::temp_dir().join(format!("ambient-js-{}.wasm", std::process::id()));
    build(&module);
    let ran = Process::new("node")
        .arg("--test")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("js/host.test.js"))
        .env("AMBIENT_MODULE", &module)
        .env("AMBIENT_PACKAGE", PACKAGE)
        .output();
    std::fs::remove_file(&module).unwrap();

    let ran = ran.expect("the development shell has `node`");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
}
