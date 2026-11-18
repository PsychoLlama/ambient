//! The JavaScript target (`packages/js`) builds a program, and its host
//! runs it under Node. See "JavaScript hosts" in `spec/targets.md`.
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
use ambient::project::{Disk, Files};
use ambient::store::MemoryStore;
use ambient::symbol::ModuleId;
use ambient::syntax::parse;
use ambient::workspace::Workspace;

/// The package the tests run: its ID, and its one module. Its target
/// binds an ability that answers at once and one that answers later.
/// What it is handed is how long it waits before it gives up, and gives
/// zero. Two externs are functions of the package's JavaScript: one
/// calls a function it is handed, and one performs an ability of its
/// row. The rest have none: each is bound to something JavaScript has,
/// so the build writes it, and one gives a handle another takes.
const PACKAGE: &str = "app";
const PROGRAM: &str = r#"
    import ambient.iterator;
    import js.{binding, host};

    ability say(text: string): null;
    ability sleep(milliseconds: int): null;

    extern handler said for say;
    extern suspending handler slept for sleep;

    extern fn applied(work: fn(int): int, to: int): int;
    extern fn echoed(text: string): null with say;
    @binding("Math.max") extern fn larger(one: int, other: int): int;

    brand Address;
    @binding({ new: "URL" }) extern fn address(text: string): Address;
    @binding({ property: "hostname" }) extern fn host_of(address: Address): string;
    @binding({ method: "toString" }) extern fn written(address: Address): string;

    target page extends host { say: said, sleep: slept }

    fn main(patience: int): int with say, sleep {
        return first(fn() => together(), fn() => gives_up(patience));
    }

    fn together(): int with say, sleep {
        let (counted, waited) = all(fn() => count(3), fn() => linger());
        return applied(fn(count: int) => count + waited, counted);
    }

    fn gives_up(after: int): int with sleep {
        sleep!(after);
        let home = address("https://example.com/path");
        return larger(0, -1) + host_of(home).length() + written(home).length() - 35;
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

/// The JavaScript of the package's externs, which its build takes:
/// a page that says nothing and waits as long as it is asked to.
const HOSTED: &str = r#"
    export const said = () => null;
    export const slept = (ms) => new Promise((wake) => setTimeout(wake, ms, null));
    export const applied = (work, to) => work(to);
    export const echoed = (line, call) => call.perform("app", "main.say", line);
"#;

/// The JavaScript target, as it is in this repository.
fn target() -> BTreeMap<PathBuf, String> {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/js");
    let mut files = BTreeMap::new();
    for path in Disk.sources(&package).unwrap() {
        let text = Disk.read(&package.join(&path)).unwrap();
        files.insert(Path::new("js").join(path), text);
    }
    files
}

/// Builds `main` of [`PROGRAM`] under its target into the directory
/// `output`: the module, and the JavaScript that loads it.
fn build(output: &Path) {
    let manifest = format!(
        "name = \"app\"\nid = \"{PACKAGE}\"\nversion = \"0.1.0\"\nfiles = [\"js\"]\n\
         [dependencies]\njs = {{ id = \"7c1e4b9a02d84f6e9b35a1c8d07e6f24\", path = \"../js\" }}"
    );
    let mut files = target();
    for (path, text) in [
        ("app/ambient.toml", manifest.as_str()),
        ("app/src/main.ab", PROGRAM),
        ("app/js/main.js", HOSTED),
    ] {
        files.insert(PathBuf::from(path), text.to_owned());
    }
    let session = Session {
        files: &files,
        store: &MemoryStore::new(),
        snapshots: &ambient::project::Nowhere,
        world: Rc::new(RefCell::new(System::new(Vec::new()))),
        launcher: None,
    };
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(output.to_owned()),
        target: Some("main.page".to_owned()),
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
    let built = std::env::temp_dir().join(format!("ambient-js-{}", std::process::id()));
    build(&built);
    let (open, unit) = (built.join("open.wasm"), built.join("unit.wasm"));
    build_to_load(&open, &unit);
    let ran = Process::new("node")
        .arg("--test")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("js/host.test.js"))
        .env("AMBIENT_BUILD", &built)
        .env("AMBIENT_OPEN", &open)
        .env("AMBIENT_UNIT", &unit)
        .env("AMBIENT_PACKAGE", PACKAGE)
        .output();
    std::fs::remove_dir_all(built).unwrap();

    let ran = ran.expect("the development shell has `node`");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
}
