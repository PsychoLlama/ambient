//! A program linked with a native bundle by the machine's own linker.
//! See "Native code" in `docs/spec/linking.md`.
//!
//! These tests compile a bundle written in C with the machine's `cc`,
//! or one in Rust with `rustc`, under the system's temporary directory,
//! and remove what they wrote.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use ambient::backend::native::MACHINE;
use ambient::cli::{Chosen, Command, Machine, Session, Unlinked, built, symbol};
use ambient::host::System;
use ambient::project::{Disk, Nowhere};
use ambient::store::MemoryStore;
use support::scripted::Scripted;

const ID: &str = "0123456789abcdef0123456789abcdef";

/// A package whose `main` gives what a function of its bundle gives.
/// Nothing reaches `absent`, and the bundle has no function for it.
/// Its target `world` binds an ability to a handler of the bundle,
/// and another to one a thread of the bundle answers.
const SOURCE: &str = "use system.{cli};

extern fn double(n: int): int;
extern fn absent(n: int): int;

fn unused(): int => absent(1);

pub fn main(): int => double(21);

pub ability triple(n: int): int;
extern handler tripled for triple;
pub ability later(n: int): int;
extern handler(suspending) answered for later;
pub target world extends cli { triple: tripled, later: answered }
pub target apart { triple: tripled }

pub fn tripling(): int with triple => triple!(14);

pub fn waiting(): int with later => later!(40) + later!(1) + 1;

pub fn stranded(): int with triple => tripling();
";

/// The binaries of the package that are run under a target of its
/// own: two under one that extends the command line's, and one under
/// one that does not.
const BINS: &str = "[bin.tripling]\ntarget = \"pkg.main.world\"\n\
                    [bin.waiting]\ntarget = \"pkg.main.world\"\n\
                    [bin.stranded]\ntarget = \"pkg.main.apart\"\n";

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

/// The header `ambient header` says for the package.
fn header(scratch: &Scratch) -> String {
    let world = Rc::new(RefCell::new(Scripted::default()));
    let session = Session {
        directory: Path::new("."),
        files: &Disk,
        store: &MemoryStore::new(),
        snapshots: &Nowhere,
        world: world.clone(),
        launcher: None,
        linker: &Unlinked,
    };
    let path = scratch.0.join("app");
    assert_eq!(
        session.execute(&Command::Header {
            at: Chosen::at(path)
        }),
        0
    );
    world.borrow().out.clone()
}

/// A bundle for the package, in C, written against its header:
/// each function reads its argument where it lies and makes what it
/// gives, through the runtime. `told` is what its author made of the
/// header.
fn bundle(scratch: &Scratch, told: fn(String) -> String) {
    let (double, tripled) = (symbol(ID, "main.double"), symbol(ID, "main.tripled"));
    let answered = symbol(ID, "main.answered");
    let source = format!(
        "#include \"ambient.h\"
#include <pthread.h>
#include <stdlib.h>
struct wait {{ uint64_t ticket; int64_t number; }};
static uint64_t made(void *data) {{
  struct wait *wait = data;
  uint64_t answer = amb_int_new(wait->number);
  free(wait);
  return answer;
}}
static void *answering(void *data) {{
  struct wait *wait = data;
  if (!amb_bundle_waiting(wait->ticket) || !amb_bundle_answer(wait->ticket, made, wait))
    free(wait);
  return 0;
}}
uint64_t {answered}(struct amb_call *call, uint64_t args) {{
  struct wait *wait = malloc(sizeof *wait);
  pthread_t thread;
  wait->number = amb_int_value(amb_word_at(args, 0));
  wait->ticket = amb_bundle_later(call);
  pthread_create(&thread, 0, answering, wait);
  pthread_detach(thread);
  return AMB_NULL;
}}
uint64_t {double}(struct amb_call *call, uint64_t args) {{
  return amb_int_new(amb_int_value(amb_word_at(args, 0)) * 2);
}}
uint64_t {tripled}(struct amb_call *call, uint64_t args) {{
  return amb_int_new(amb_int_value(amb_word_at(args, 0)) * 3);
}}
"
    );
    scratch.write("app/native/ambient.h", &told(header(scratch)));
    let source = scratch.write("app/native/app.c", &source);
    let compiled = Process::new("cc")
        .args(["-c", "-ffunction-sections", "-o"])
        .arg(scratch.0.join("app/native/app.o"))
        .arg(source)
        .status();
    assert!(compiled.unwrap().success());
}

/// A bundle for the package in Rust, as an archive: it holds the
/// standard library of its language, as the launcher does, and its
/// `double` is worked out on a thread of that library.
fn archive(scratch: &Scratch) {
    let source = format!(
        "#![allow(non_upper_case_globals)]
unsafe extern \"C\" {{
    fn amb_int_new(value: i64) -> u64;
    fn amb_int_value(value: u64) -> i64;
    fn amb_word_at(value: u64, index: u64) -> u64;
}}
#[unsafe(no_mangle)]
pub static {built}: u8 = 0;
#[unsafe(no_mangle)]
pub extern \"C\" fn {double}(_call: *mut u8, args: u64) -> u64 {{
    let number = unsafe {{ amb_int_value(amb_word_at(args, 0)) }};
    let doubled = std::thread::spawn(move || number * 2).join();
    unsafe {{ amb_int_new(doubled.unwrap_or(0)) }}
}}
",
        built = built(ID),
        double = symbol(ID, "main.double"),
    );
    let source = scratch.write("app/native/app.rs", &source);
    let compiled = Process::new("rustc")
        .args(["--edition", "2024", "--crate-type", "staticlib", "-o"])
        .arg(scratch.0.join("app/native/libapp.a"))
        .arg(source)
        .status();
    assert!(compiled.unwrap().success());
}

/// What `command` gives of the package and its bundle, written under
/// `scratch`.
fn executed(scratch: &Scratch, command: &Command) -> i32 {
    executed_told(scratch, command, |header| header)
}

/// The same, of a bundle whose author was told `told` of the header.
fn executed_told(scratch: &Scratch, command: &Command, told: fn(String) -> String) -> i32 {
    package(scratch, "native/app.o");
    bundle(scratch, told);
    executed_of(scratch, command)
}

/// Writes the package, whose bundle is the file at `path` in it.
fn package(scratch: &Scratch, path: &str) {
    let manifest = format!(
        "[package]\nname = \"app\"\nid = \"{ID}\"\nversion = \"0.1.0\"\n\n{BINS}\n\
         [native.{MACHINE}]\npath = \"{path}\"\n"
    );
    scratch.write("app/ambient.toml", &manifest);
    scratch.write("app/src/main.ab", SOURCE);
}

/// What `command` gives of the package and bundle written under
/// `scratch`.
fn executed_of(scratch: &Scratch, command: &Command) -> i32 {
    let session = Session {
        directory: Path::new("."),
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
    let output = scratch.0.join("made/here/out");
    let build = Command::Build {
        at: Chosen::at(scratch.0.join("app")),
        bin: "main".to_owned(),
        output: Some(output.clone()),
    };

    assert_eq!(executed(&scratch, &build), 0);
    assert_eq!(Process::new(output).status().unwrap().code(), Some(42));
}

#[test]
fn a_program_that_reaches_a_bundle_is_run_by_what_was_linked_with_it() {
    let scratch = Scratch::new();
    let run = Command::Run {
        at: Chosen::at(scratch.0.join("app")),
        bin: "main".to_owned(),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 42);
}

#[test]
fn a_target_of_the_package_binds_a_handler_of_its_bundle() {
    let scratch = Scratch::new();
    let run = Command::Run {
        at: Chosen::at(scratch.0.join("app")),
        bin: "tripling".to_owned(),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 42);
}

#[test]
fn a_thread_of_a_bundle_answers_a_task_that_waits() {
    let scratch = Scratch::new();
    let run = Command::Run {
        at: Chosen::at(scratch.0.join("app")),
        bin: "waiting".to_owned(),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 42);
}

#[test]
fn a_program_is_not_run_under_a_target_that_is_no_command_lines() {
    let scratch = Scratch::new();
    let run = Command::Run {
        at: Chosen::at(scratch.0.join("app")),
        bin: "stranded".to_owned(),
        args: Vec::new(),
    };

    assert_eq!(executed(&scratch, &run), 1);
}

/// The launcher holds the same library, and each keeps its own.
#[test]
fn a_bundle_that_holds_its_languages_library_is_linked() {
    let scratch = Scratch::new();
    let run = Command::Run {
        at: Chosen::at(scratch.0.join("app")),
        bin: "main".to_owned(),
        args: Vec::new(),
    };

    package(&scratch, "native/libapp.a");
    archive(&scratch);
    assert_eq!(executed_of(&scratch, &run), 42);
}

/// Sealed, such a bundle would use its own runtime and say nothing.
#[test]
fn a_bundle_that_brings_a_runtime_of_its_own_is_not_linked() {
    let scratch = Scratch::new();
    let build = Command::Build {
        at: Chosen::at(scratch.0.join("app")),
        bin: "main".to_owned(),
        output: Some(scratch.0.join("out")),
    };
    let brings = |header: String| header + "int64_t amb_int_value(uint64_t value) { return 0; }\n";

    assert_eq!(executed_told(&scratch, &build, brings), 1);
}

#[test]
fn a_bundle_built_for_another_layout_is_not_linked() {
    let scratch = Scratch::new();
    let build = Command::Build {
        at: Chosen::at(scratch.0.join("app")),
        bin: "main".to_owned(),
        output: Some(scratch.0.join("out")),
    };
    let other = |header: String| header.replace(&built(ID), "amb_built_for_another");

    assert_eq!(executed_told(&scratch, &build, other), 1);
}
