//! The canonical `Option`/`Result` declarations live in Ambient source
//! (`core_lib/option.ab`, `core_lib/result.ab`), while the type checker's
//! prelude is built from the engine-side spec (`PRELUDE_ENUMS`) because
//! the engine cannot parse. `validate_reserved_declaration` pins the two
//! together at build time; this test pins them at test time — parse the
//! embedded sources and check the declarations are present, reserved,
//! and canonical.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use ambient_engine::ast::ItemKind;
use ambient_engine::core_library::CoreLibrary;
use ambient_engine::infer::enums::validate_reserved_declaration;
use ambient_engine::types::{OPTION_UUID, RESULT_UUID};
use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Write a single-`main.ab` package and run it through the real `ambient`
/// binary, returning trimmed stdout on success.
fn run_main(source: &str) -> String {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"prelude_enums\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(src.join("main.ab"), source).expect("write main");
    run(dir.path())
}

fn run(dir: &Path) -> String {
    let output = Command::new(ambient_bin())
        .arg("run")
        .arg(dir)
        .output()
        .expect("failed to spawn ambient");
    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout.trim().to_string()
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

#[test]
fn core_sources_declare_the_canonical_prelude_enums() {
    let cases = [("Option", OPTION_UUID), ("Result", RESULT_UUID)];

    for (name, uuid) in cases {
        let source = CoreLibrary::get_source(&[Arc::from(name)])
            .unwrap_or_else(|e| panic!("core module `{name}` has embedded source: {e}"));
        let module = ambient_parser::parse(source)
            .unwrap_or_else(|e| panic!("core module `{name}` parses: {e}"));

        let def = module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Enum(def) if def.name.as_ref() == name => Some(def),
                _ => None,
            })
            .unwrap_or_else(|| panic!("core module `{name}` declares enum `{name}`"));

        assert!(def.is_public, "`{name}` must be `pub`");
        assert_eq!(def.uuid, uuid, "`{name}` must carry its reserved uuid");
        validate_reserved_declaration(def)
            .unwrap_or_else(|e| panic!("`{name}` declaration drifted from the prelude spec: {e}"));
    }
}

#[test]
fn fully_qualified_option_constructs_and_runs() {
    // `core::Option::Some(10)` and `core::Option::None` — the FQN spelling
    // of the prelude enum — construct and match end-to-end (resolve → check
    // → compile → VM), exactly like the bare `Some`/`None`.
    let out = run_main(
        r"
pub fn run(): Number {
  let some = match core::Option::Some(10) { Some(n) => n, None => 0 };
  let none = match core::Option::None { Some(n) => n, None => 5 };
  some + none
}
",
    );
    assert_eq!(out, "15");
}

#[test]
fn fully_qualified_result_constructs_and_runs() {
    // `core::Result::Ok`/`core::Result::Err` — the FQN spelling — construct
    // and match end-to-end.
    let out = run_main(
        r"
pub fn run(): Number {
  let ok = match core::Result::Ok(1) { Ok(n) => n, Err(e) => e };
  let err = match core::Result::Err(2) { Ok(n) => n, Err(e) => e };
  ok + err
}
",
    );
    assert_eq!(out, "3");
}
