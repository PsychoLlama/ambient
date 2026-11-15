//! `ambient build`: an executable of a program. See "Building" in
//! `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command as Process;
use std::rc::Rc;

use ambient::cli::{Command, carried, start};
use ambient::host::World;
use support::scripted::{LAUNCHER, Scripted, app};

/// Builds `main` of a package whose one module is `source`, where the
/// command's own file holds `launcher`, and gives the file it made.
fn build(source: &str, launcher: &[u8]) -> Vec<u8> {
    let world = Scripted {
        files: [(LAUNCHER.to_owned(), launcher.to_vec())].into(),
        ..Scripted::default()
    };
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: None,
    };
    let (code, after) = world.execute(&app(source), &command);
    assert_eq!(
        (code, after.out.as_str()),
        (0, "built app\n"),
        "{}",
        after.err
    );
    after.files["app"].clone()
}

/// Starts what an executable carries in `world`, as the command does
/// when its own file is one. Gives the code and the world it left.
fn started(executable: &[u8], world: Scripted) -> (i32, Scripted) {
    let program = carried(&mut Cursor::new(executable)).unwrap().unwrap();
    let scripted = Rc::new(RefCell::new(world));
    let world: Rc<RefCell<dyn World>> = scripted.clone();
    let code = start(&program, &world);
    let after = scripted.borrow().clone();
    (code, after)
}

const GREETS: &str = r#"
    import system.console;
    import system.env;

    fn main(): int with console.print, env.args {
        console.print!("hello, " ++ env.args!().join_with(" and "));
        return 3;
    }
"#;

#[test]
fn an_executable_is_the_command_and_then_the_program() {
    let executable = build(GREETS, b"the command");
    assert!(executable.starts_with(b"the command"));

    let world = Scripted {
        args: vec!["Ada".to_owned(), "--help".to_owned()],
        ..Scripted::default()
    };
    let (code, after) = started(&executable, world);
    assert_eq!((code, after.out.as_str()), (3, "hello, Ada and --help\n"));
}

#[test]
fn a_file_that_carries_nothing_is_only_the_command() {
    for file in [&b""[..], b"short", b"a command and nothing after it"] {
        assert_eq!(carried(&mut Cursor::new(file)).unwrap(), None);
    }
}

#[test]
fn a_fault_in_an_executable_is_reported_where_it_was_written() {
    let source = "
        fn divide(by: int): int => 10 / by;
        fn main(): int {
            let result = divide(0);
            return result;
        }
    ";

    let (code, after) = started(&build(source, b""), Scripted::default());
    assert_eq!(code, 1);
    assert_eq!(
        after.err,
        "error: divide_by_zero: an integer was divided by zero\n  in app:main.divide at app/src/main.ab:2:39\n  in app:main.main at app/src/main.ab:4:26\n"
    );
}

#[test]
fn an_executable_packs_its_own_code_and_loads_it() {
    let source = r#"
        import system.code;
        import system.console;

        fn other(name: string): string => "not that function";
        fn greet(name: string): string => "hello, ${name}";

        fn main(): null with code, console.print {
            match code.load!(code.pack!(greet) as bytes else 0x[]) {
                let error: Error => { console.print!(error.message); }
                let loaded: unknown => {
                    let greet = loaded as fn(string): string else other;
                    console.print!(greet("ada"));
                }
            }
        }
    "#;

    let (code, after) = started(&build(source, b""), Scripted::default());
    assert_eq!(
        (code, after.out.as_str()),
        (0, "hello, ada\n"),
        "{}",
        after.err
    );
}

#[test]
fn a_program_that_is_wrong_builds_nothing() {
    let world = Scripted {
        files: [(LAUNCHER.to_owned(), Vec::new())].into(),
        ..Scripted::default()
    };
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("out")),
    };

    let (code, after) = world.execute(&app("fn main(): int => \"one\";"), &command);
    assert_eq!(code, 1);
    assert!(
        after.err.starts_with("error: app/src/main.ab:1:19"),
        "{}",
        after.err
    );
    assert!(!after.files.contains_key("out"));
}

/// The one test that touches the disk: the command builds a package
/// there, and what it built runs with the package gone.
#[test]
fn the_command_builds_a_file_that_runs_on_its_own() {
    let ambient = env!("CARGO_BIN_EXE_ambient");
    let directory = std::env::temp_dir().join(format!("ambient-build-{}", std::process::id()));
    let package = directory.join("app");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(package.join("ambient.toml"), "name = \"app\"\nid = \"app\"").unwrap();
    std::fs::write(package.join("src/main.ab"), GREETS).unwrap();
    let executable = directory.join("greet");

    let build = Process::new(ambient)
        .args(["build", "app", "--output", "greet"])
        .current_dir(&directory)
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::remove_dir_all(&package).unwrap();
    let ran = Process::new(&executable)
        .args(["build", "you"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(ran.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "hello, build and you\n"
    );
}
