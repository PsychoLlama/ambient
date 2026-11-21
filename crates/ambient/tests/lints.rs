//! Lints: the functions a manifest lists, which checking a package
//! runs for what each finds wrong. See "Commands" in `docs/spec/tools.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use support::scripted::{Scripted, check};

/// A lint that wants every function of what it covers named in more
/// than one letter, and the functions it is run over.
const SHORT: &str = r#"
    use ambient.build;
    use ambient.build.{Diagnostic, Diagnostics, Module, Selection};
    use ambient.tree.{Reference};

    fn named(found: Diagnostics, of: Reference): Diagnostics =>
        of.name.length() > 1 ? found : found.push({ on: of, message: "`${of.name}` says nothing" });

    fn short(of: Selection): Diagnostics with build.definitions {
        return of.modules.fold([], fn(found: Diagnostics, module: Module): Diagnostics {
            let defined = cast::<List<Reference>>(build.definitions!(module), []);
            return defined.fold(found, named);
        });
    }

    fn f(): int => 1;
    fn fine(): int => 2;
"#;

/// Checks a package whose module is `source` and whose manifest ends
/// with `rest`, and gives what was reported.
fn checked(source: &str, rest: &str) -> (i32, Scripted) {
    let manifest = format!("[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n{rest}");
    let files: BTreeMap<PathBuf, String> = [
        ("app/ambient.toml", manifest),
        ("app/src/main.ab", source.to_owned()),
    ]
    .into_iter()
    .map(|(path, text)| (PathBuf::from(path), text))
    .collect();
    check(&files)
}

const LISTED: &str = "lints = [\"pkg.main.short\"]";

#[test]
fn checking_reports_what_a_listed_lint_finds_where_it_is() {
    let (code, world) = checked(SHORT, LISTED);

    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: app/src/main.ab:16:5: app:main.f: `f` says nothing (app:main.short)\n"
    );
}

#[test]
fn a_lint_nothing_lists_is_not_run() {
    let (code, world) = checked(SHORT, "");

    assert_eq!((code, world.err.as_str()), (0, ""));
}

#[test]
fn a_lint_is_not_run_over_what_does_not_check() {
    let broken = format!("{SHORT}\nfn g(): int => missing();");

    let (_, world) = checked(&broken, LISTED);

    assert!(world.err.contains("missing"), "{}", world.err);
    assert!(!world.err.contains("says nothing"), "{}", world.err);
}

#[test]
fn what_is_listed_as_a_lint_takes_a_selection_and_gives_diagnostics() {
    let (_, world) = checked(SHORT, "lints = [\"pkg.main.fine\"]");

    assert!(
        world
            .err
            .contains("app:main.fine: a lint takes a `build.Selection`"),
        "{}",
        world.err
    );
}

#[test]
fn a_lint_a_manifest_lists_is_one_the_package_reaches() {
    let (_, world) = checked(SHORT, "lints = [\"pkg.main.absent\"]");

    assert!(
        world
            .err
            .contains("the lint `pkg.main.absent` names `pkg.main.absent`"),
        "{}",
        world.err
    );
}
