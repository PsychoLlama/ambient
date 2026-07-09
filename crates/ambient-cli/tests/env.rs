//! End-to-end coverage of the `core::system::Env` ability through the
//! `ambient` binary: a package performs every `Env` method and the test
//! asserts the observable behavior — the captured argv (program path at
//! index 0, then the trailing args after `--`), a variable the test set
//! in the child's environment, and non-degenerate `cwd`/`pid`.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Strip ANSI escape sequences (colors) from output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for t in chars.by_ref() {
                    if t.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A package whose `run` exercises every `Env` method, printing one
/// labeled line per observation so the test can assert on each.
const ENV_PROGRAM: &str = r#"pub fn run(): () with core::system::Env, core::system::Stdio {
  let args = core::system::Env::args!();
  core::system::Stdio::out!("argc " + core::convert::to_string(args.length()));
  core::system::Stdio::out!("arg1 " + args.get(1).unwrap_or("?"));
  core::system::Stdio::out!("arg2 " + args.get(2).unwrap_or("?"));
  core::system::Stdio::out!("var " + core::system::Env::var!("AMBIENT_ENV_IT").unwrap_or("unset"));
  core::system::Stdio::out!("missing " + core::system::Env::var!("AMBIENT_ENV_ABSENT").unwrap_or("unset"));
  core::system::Stdio::out!("cwdlen " + core::convert::to_string(core::system::Env::cwd!().unwrap_or("").length()));
  core::system::Stdio::out!("pid " + core::convert::to_string(core::system::Env::pid!()));
}
"#;

/// Write a single-file package and return its directory.
fn package(source: &str) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"env_it\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(src.join("main.ab"), source).expect("write main.ab");
    dir
}

/// The labeled line's value, e.g. `line_value(lines, "arg1")` -> "a".
fn line_value<'a>(lines: &'a [&'a str], label: &str) -> &'a str {
    let prefix = format!("{label} ");
    lines
        .iter()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no `{label}` line in output: {lines:?}"))
}

#[test]
fn env_methods_observe_the_process_environment() {
    let dir = package(ENV_PROGRAM);

    let output = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .arg("--")
        .arg("a")
        .arg("b")
        // A variable set in the child's environment must be readable via
        // `Env::var`; an unset one reads as None (`unwrap_or("unset")`).
        .env("AMBIENT_ENV_IT", "hello")
        .env_remove("AMBIENT_ENV_ABSENT")
        .output()
        .expect("failed to spawn ambient");

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();

    // argv: index 0 is the program path (the dir we passed), so argc is 3
    // and the user args land at indices 1 and 2.
    assert_eq!(line_value(&lines, "argc"), "3", "program path + two args");
    assert_eq!(line_value(&lines, "arg1"), "a");
    assert_eq!(line_value(&lines, "arg2"), "b");

    // A set var resolves; an unset var is None.
    assert_eq!(line_value(&lines, "var"), "hello");
    assert_eq!(line_value(&lines, "missing"), "unset");

    // cwd is a non-empty path.
    assert_ne!(line_value(&lines, "cwdlen"), "0", "cwd should be non-empty");

    // pid is a positive number.
    let pid: f64 = line_value(&lines, "pid")
        .parse()
        .expect("pid line is numeric");
    assert!(pid > 0.0, "pid should be positive, got {pid}");
}

/// With no trailing args, argv is just the program path at index 0.
#[test]
fn args_without_trailing_args_is_just_the_program_path() {
    let dir = package(ENV_PROGRAM);

    let output = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .env_remove("AMBIENT_ENV_IT")
        .output()
        .expect("failed to spawn ambient");

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(output.status.success(), "run failed:\n{stdout}");

    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
    assert_eq!(line_value(&lines, "argc"), "1", "only the program path");
    // The user args are absent, so the fallbacks show.
    assert_eq!(line_value(&lines, "arg1"), "?");
    assert_eq!(line_value(&lines, "var"), "unset");
}
