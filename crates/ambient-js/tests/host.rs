//! The JavaScript host (`js/host.js`) runs a program built for it, under
//! Node. See "JavaScript hosts" in `spec/targets.md`.
//!
//! The tests themselves are JavaScript (`js/*.test.js`), since what they test
//! is. This builds the program they run and runs them, so that the one
//! command that runs every test runs these too.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;

use ambient::backend::wasm::{self, Wasm};
use ambient::cli::{Command, Session};
use ambient::engine::Engine;
use ambient::host::System;
use ambient::store::MemoryStore;
use ambient::symbol::ModuleId;
use ambient::syntax::parse;
use ambient::workspace::Workspace;

/// The package the tests run: its ID, and its one module. Its target
/// binds an ability that answers at once and one that answers later.
/// What it is handed is how long it waits before it gives up, and gives
/// zero. Two externs are functions: one calls a function it is handed,
/// and one performs an ability of its row.
const PACKAGE: &str = "app";
const PROGRAM: &str = r#"
    import ambient.iterator;

    ability say(text: string): null;
    ability sleep(milliseconds: int): null;

    extern handler said for say;
    extern suspending handler slept for sleep;

    extern fn applied(work: fn(int): int, to: int): int;
    extern fn echoed(text: string): null with say;

    target page { say: said, sleep: slept }

    fn main(patience: int): int with say, sleep {
        return first(fn() => together(), fn() => gives_up(patience));
    }

    fn together(): int with say, sleep {
        let (counted, waited) = all(fn() => count(3), fn() => linger());
        return applied(fn(count: int) => count + waited, counted);
    }

    fn gives_up(after: int): int with sleep {
        sleep!(after);
        return 0;
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
        echoed("still here");
        return 1;
    }
"#;

/// Builds `main` of [`PROGRAM`] under its target as a module at `output`.
fn build(output: &Path) {
    let file = |(path, text): (&str, String)| (PathBuf::from(path), text);
    let manifest = format!("name = \"app\"\nid = \"{PACKAGE}\"\nversion = \"0.1.0\"");
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

/// A program that code is loaded into, which no command builds yet:
/// it calls with what it is handed the function its page fetches.
const LOADER: &str = r"
    ability fetch(): unknown;

    extern suspending handler fetched for fetch;

    target page { fetch: fetched }

    fn main(number: int): int with fetch {
        let fetched = fetch!() as fn(int): int else fn(number: int) => 0 - 1;
        return fetched(number);
    }
";

/// What is loaded into it: a module the program does not reach.
const PLUGIN: &str = "pub fn plugin(number: int): int { return number + number; }";

/// Builds [`LOADER`] as a module at `open`, and [`PLUGIN`] at `unit` as
/// a unit told nothing of its target, which so loads into any.
fn build_to_load(open: &Path, unit: &Path) {
    let mut workspace = Workspace::new();
    ambient::library::install(&mut workspace).unwrap();
    workspace.depend(PACKAGE, "ambient", "ambient").unwrap();
    for (path, source) in [("main", LOADER), ("plugin", PLUGIN)] {
        let module = ModuleId::new(PACKAGE, path);
        workspace.set(module, parse(source).unwrap()).unwrap();
    }
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let main = ModuleId::new(PACKAGE, "main");
    let root = engine.program(&main.value("main"), &main.value("page"));
    let program = engine.link_open(&root.unwrap(), &Wasm).unwrap();
    std::fs::write(open, wasm::open_module(&program).unwrap()).unwrap();
    let plugin = ModuleId::new(PACKAGE, "plugin").value("plugin");
    let plugin = engine.link_to_load(&plugin, &Wasm).unwrap();
    std::fs::write(unit, wasm::unit(&plugin, &BTreeSet::new()).unwrap()).unwrap();
}

#[test]
fn the_host_runs_a_program_built_for_it() {
    let file = |name: &str| {
        std::env::temp_dir().join(format!("ambient-js-{}-{name}.wasm", std::process::id()))
    };
    let (module, open, unit) = (file("module"), file("open"), file("unit"));
    build(&module);
    build_to_load(&open, &unit);
    let ran = Process::new("node")
        .arg("--test")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("js/host.test.js"))
        .env("AMBIENT_MODULE", &module)
        .env("AMBIENT_OPEN", &open)
        .env("AMBIENT_UNIT", &unit)
        .env("AMBIENT_PACKAGE", PACKAGE)
        .output();
    for file in [module, open, unit] {
        std::fs::remove_file(file).unwrap();
    }

    let ran = ran.expect("the development shell has `node`");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
}
