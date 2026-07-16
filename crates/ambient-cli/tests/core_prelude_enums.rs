//! The canonical `Option`/`Result` declarations live in Ambient source
//! (`core_lib/Option.ab`, `core_lib/Result.ab`) and reach the checker and
//! compiler through the module system (via the prelude), like any other
//! enum — no hardcoded seed. `validate_reserved_declaration` pins them to
//! their reserved identity at build time; these tests pin them at test time —
//! parse the embedded sources and check the declarations are present,
//! reserved, and canonical, and exercise construction end-to-end.

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
    // Module files are snake_case; the enums they declare stay PascalCase.
    let cases = [
        ("option", "Option", OPTION_UUID),
        ("result", "Result", RESULT_UUID),
    ];

    for (module_name, name, uuid) in cases {
        let source = CoreLibrary::get_source(&[Arc::from(module_name)])
            .unwrap_or_else(|e| panic!("core module `{module_name}` has embedded source: {e}"));
        let module = ambient_parser::parse(source)
            .unwrap_or_else(|e| panic!("core module `{module_name}` parses: {e}"));

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

/// Building the core library must succeed with the prelude injection
/// enabled — the regression guard for the `core::option ↔ core::primitives`
/// cycle the separate `prelude_items` tier is designed to avoid. A cyclic
/// compile order would error here (`compilation_order` can't topo-sort a
/// cycle), and a missing prelude default would leave core modules unable to
/// resolve bare `Some`/`None`/`Number`.
#[test]
fn core_library_builds_acyclically_with_prelude() {
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    let mut module_function_hashes = std::collections::HashMap::new();
    ambient_engine::build::compile_core_modules(&mut registry, &mut module_function_hashes, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core library builds acyclically");

    // The default prelude is set as a side effect of registering core.
    let prelude = registry
        .prelude()
        .expect("core registers a default prelude");
    assert_eq!(prelude.to_string(), "core::prelude");
}

/// The `core::prelude` source must re-export the full global set: dropping
/// any name here would silently remove it from every module's scope. Pin the
/// exact set of exported leaves.
#[test]
fn prelude_reexports_the_full_global_set() {
    let source = CoreLibrary::get_source(&[Arc::from("prelude")])
        .expect("core module `prelude` has embedded source");
    let module = ambient_parser::parse(source).expect("core module `prelude` parses");

    let mut exported: Vec<String> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Use(use_def) => Some(use_def),
            _ => None,
        })
        .flat_map(|use_def| {
            assert!(use_def.is_public, "every prelude `use` must be `pub use`");
            // The re-exported name is the alias, else the final path segment.
            use_def
                .alias
                .as_ref()
                .map(|(name, _)| name.to_string())
                .or_else(|| use_def.path.last().map(|(name, _)| name.to_string()))
        })
        .collect();
    exported.sort();

    let mut expected = vec![
        "Option",
        "Some",
        "None",
        "Result",
        "Ok",
        "Err",
        "Bool",
        "Number",
        "String",
        "Binary",
        // The generic containers.
        "List",
        "Map",
        "Set",
        // The language-level error-signalling ability, plus its one method
        // as an ability-method re-export: `throw!(…)` works bare in every
        // module (handler arms still spell `Exception::throw`). `Error` is
        // the bound on `throw` — what a value must be to be thrown — so it
        // rides alongside.
        "Exception",
        "Error",
        "throw",
        // Operator traits (`Default` is deliberately excluded from the prelude).
        "Add",
        "Sub",
        "Mul",
        "Div",
        "Mod",
        "Eq",
        "Ord",
        // `Show` — no operator, but the conventional stringifier, so it
        // stays on the prelude as rendering vocabulary.
        "Show",
        // The conversion pairs — `x.into()` / `x.try_into()` dispatch and
        // the From-satisfies-Into / TryFrom-satisfies-TryInto bridges
        // anchor on their reserved identities.
        "From",
        "Into",
        "TryFrom",
        "TryInto",
        // The `System` ability set — every platform ability under one name,
        // the only member of `core::system` on the prelude (the individual
        // platform abilities stay namespaced).
        "System",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(
        exported, expected,
        "core::prelude must re-export exactly the global set"
    );
}

#[test]
fn bare_prelude_enums_construct_without_an_import() {
    // Bare `Some`/`Ok` construction with no `use` of Option/Result must
    // compile and run: the prelude enums reach the *compiler* through the
    // same import channel as the checker (`build_imported_enums` reads
    // `resolve_imports`), so nothing hardcodes them. This is the end-to-end
    // proof that deleting the compiler's `PRELUDE_ENUMS` seed is sound.
    let out = run_main(
        r"
pub fn run(): Number {
  let some = match Some(4) { Some(n) => n, None => 0 };
  let ok = match Ok(3) { Ok(n) => n, Err(e) => e };
  some + ok
}
",
    );
    assert_eq!(out, "7");
}

#[test]
fn fully_qualified_option_constructs_and_runs() {
    // `core::option::Some(10)` and `core::option::None` — the FQN spelling
    // of the prelude enum — construct and match end-to-end (resolve → check
    // → compile → VM), exactly like the bare `Some`/`None`.
    let out = run_main(
        r"
pub fn run(): Number {
  let some = match core::option::Some(10) { Some(n) => n, None => 0 };
  let none = match core::option::None { Some(n) => n, None => 5 };
  some + none
}
",
    );
    assert_eq!(out, "15");
}

#[test]
fn fully_qualified_result_constructs_and_runs() {
    // `core::result::Ok`/`core::result::Err` — the FQN spelling — construct
    // and match end-to-end.
    let out = run_main(
        r"
pub fn run(): Number {
  let ok = match core::result::Ok(1) { Ok(n) => n, Err(e) => e };
  let err = match core::result::Err(2) { Ok(n) => n, Err(e) => e };
  ok + err
}
",
    );
    assert_eq!(out, "3");
}
