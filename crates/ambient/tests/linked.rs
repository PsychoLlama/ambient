//! A program linked with a native bundle by the machine's own linker.
//! See "Native code" in `docs/spec/linking.md`.
//!
//! These tests compile a bundle written in C with the machine's `cc`,
//! under the system's temporary directory, and remove what they wrote.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use ambient::backend::native::MACHINE;
use ambient::cli::{Command, Machine, Session, symbol};
use ambient::host::System;
use ambient::project::{Disk, Nowhere};
use ambient::store::MemoryStore;

const ID: &str = "0123456789abcdef0123456789abcdef";

/// A package whose `main` gives what a function of its bundle gives.
/// Nothing reaches `absent`, and the bundle has no function for it.
/// Its target `world` binds an ability to a handler of the bundle.
const SOURCE: &str = "use system.{cli};

extern fn double(n: int): int;
extern fn absent(n: int): int;

fn unused(): int => absent(1);

pub fn main(): int => double(21);

pub ability triple(n: int): int;
extern handler tripled for triple;
pub target world extends cli { triple: tripled }
pub target apart { triple: tripled }

pub fn tripling(): int with triple => triple!(14);
";

/// A directory of this test's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("ambient-linked-{}-{unique}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }

    fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A bundle for the package, in C: `double` reads its argument where
/// it lies and makes what it gives, through the runtime.
fn bundle(scratch: &Scratch) {
    let (double, tripled) = (symbol(ID, "main.double"), symbol(ID, "main.tripled"));
    let source = format!(
        "#include <stdint.h>
struct amb_call;
uint64_t amb_word_at(uint64_t, uint64_t);
int64_t amb_int_value(uint64_t);
uint64_t amb_int_new(int64_t);
uint64_t {double}(struct amb_call *call, uint64_t args) {{
  return amb_int_new(amb_int_value(amb_word_at(args, 0)) * 2);
}}
uint64_t {tripled}(struct amb_call *call, uint64_t args) {{
  return amb_int_new(amb_int_value(amb_word_at(args, 0)) * 3);
}}
"
    );
    let source = scratch.write("app/native/app.c", &source);
    let compiled = Process::new("cc")
        .args(["-c", "-ffunction-sections", "-o"])
        .arg(scratch.0.join("app/native/app.o"))
        .arg(source)
        .status();
    assert!(compiled.unwrap().success());
}

/// What `command` gives of the package and its bundle, written under
/// `scratch`.
fn executed(scratch: &Scratch, command: &Command) -> i32 {
    let manifest = format!(
        "name = \"app\"\nid = \"{ID}\"\nversion = \"0.1.0\"\n\n\
         [native.{MACHINE}]\npath = \"native/app.o\"\n"
    );
    scratch.write("app/ambient.toml", &manifest);
    scratch.write("app/src/main.ab", SOURCE);
    bundle(scratch);
    let session = Session {
        files: &Disk,
        store: &MemoryStore::new(),
        snapshots: &Nowhere,
        world: Rc::new(RefCell::new(System::new(Vec::new()))),
        launcher: None,
        linker: &Machine::under(scratch.0.join("linked")),
    };
    session.execute(command)
}

#[test]
fn an_executable_runs_the_function_of_a_bundle_its_program_reaches() {
    let scratch = Scratch::new();
    let output = scratch.0.join("out");
    let build = Command::Build {
        path: scratch.0.join("app"),
        entry: "main.main".to_owned(),
        output: Some(output.clone()),
        target: None,
    };

    assert_eq!(executed(&scratch, &build), 0);
    assert_eq!(Process::new(output).status().unwrap().code(), Some(42));
}

#[test]
fn a_program_that_reaches_a_bundle_is_run_by_what_was_linked_with_it() {
    let scratch = Scratch::new();
    let run = Command::Run {
        path: scratch.0.join("app"),
        entry: "main.main".to_owned(),
        target: None,
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 42);
}

#[test]
fn a_target_of_the_package_binds_a_handler_of_its_bundle() {
    let scratch = Scratch::new();
    let run = Command::Run {
        path: scratch.0.join("app"),
        entry: "main.tripling".to_owned(),
        target: Some("main.world".to_owned()),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 42);
}

#[test]
fn a_program_is_not_run_under_a_target_that_is_no_command_lines() {
    let scratch = Scratch::new();
    let run = Command::Run {
        path: scratch.0.join("app"),
        entry: "main.tripling".to_owned(),
        target: Some("main.apart".to_owned()),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 1);
}
