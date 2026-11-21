//! What a tool is told of a program and of the world outside it. See
//! "Build abilities" in `docs/spec/tools.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;
use ambient::hash::Hash;
use ambient::project::{Commit, Snapshots, Source};
use ambient::store::MemoryStore;
use support::scripted::Scripted;

/// What every tool here starts with: a target built by `report`, which
/// each test writes to give one file of what it was told.
const HEAD: &str = r#"
    use ambient.build;
    use ambient.tree;
    use ambient.build.{Annotation, Build, Files, Module, Ran};
    use ambient.tree.{Reference};
    use ambient.Error;

    target page built by report {}

    fn main(): int => helper() + 1;
    tag note;
    /// Gives one.
    #[note("reached")] fn helper(): int => 1;
    #[note("apart")] fn unused(): int => 2;

    fn names(of: List<Reference>): string => of.map(fn(of: Reference): string => of.name).join_with(" ");
    fn file(text: string): Files => [{ path: "told.txt", bytes: text.to_bytes() }];
"#;

/// A machine that fetches `held` from anywhere, and counts how often.
struct Remote {
    held: Vec<u8>,
    asked: RefCell<usize>,
}

impl Snapshots for Remote {
    fn held(&self, _: &Hash) -> Option<PathBuf> {
        None
    }

    fn resolve(&self, _: &Source) -> Result<String, String> {
        Err("no repository".to_owned())
    }

    fn fetch(&self, _: Commit) -> Result<Hash, String> {
        Err("no repository".to_owned())
    }

    fn bytes(&self, _: &str) -> Result<Vec<u8>, String> {
        *self.asked.borrow_mut() += 1;
        Ok(self.held.clone())
    }
}

/// What the tool `report` of a package granted `grants` was told,
/// over `store` on a machine that fetches from `remote`.
fn told_from(report: &str, grants: &str, store: &MemoryStore, remote: &Remote) -> String {
    let grants: Vec<String> = grants
        .split(' ')
        .map(|grant| format!("\"ambient.build.{grant}\""))
        .collect();
    let manifest = format!(
        "name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\ngrants = [{}]",
        grants.join(", ")
    );
    let files: BTreeMap<PathBuf, String> = [
        ("app/ambient.toml", manifest),
        ("app/src/main.ab", format!("{HEAD}{report}")),
    ]
    .into_iter()
    .map(|(path, text)| (PathBuf::from(path), text))
    .collect();
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("out")),
        target: Some("main.page".to_owned()),
    };
    let world = Scripted::default()
        .execute_from(&files, store, remote, &command)
        .1;
    assert_eq!(world.err, "");
    String::from_utf8_lossy(&world.files["out/told.txt"]).into_owned()
}

fn told(report: &str, grants: &str) -> String {
    let remote = Remote {
        held: Vec::new(),
        asked: RefCell::new(0),
    };
    told_from(report, grants, &MemoryStore::new(), &remote)
}

#[test]
fn a_tool_is_told_the_modules_of_a_package_and_what_each_defines() {
    let said = told(
        r#"fn report(of: Build): Files with build.modules, build.definitions {
            let modules = build.modules!("app").map(fn(of: Module): string => of.path.join_with("."));
            let defined = cast::<List<Reference>>(build.definitions!({ package: "app", path: ["main"] }), []);
            return file("${modules.join_with(" ")}: ${names(defined)}");
        }"#,
        "modules definitions",
    );

    assert_eq!(said, "main: page main note helper unused names file report");
}

#[test]
fn a_tool_is_told_what_the_definitions_of_a_module_are_themselves_annotated_with() {
    let said = told(
        r#"fn report(of: Build): Files with build.annotated {
            let found = build.annotated!({ package: "app", path: ["main"] }, note);
            let said = fn(found: Annotation): string => "${found.on.name}=${cast::<string>(found.value, "?")}";
            return file(cast::<List<Annotation>>(found, []).map(said).join_with(" "));
        }"#,
        "annotated",
    );

    // Reached from the entry or not: a module is asked, and no walk made.
    assert_eq!(said, "helper=reached unused=apart");
}

#[test]
fn a_tool_is_told_what_a_definition_reaches_in_the_order_it_is_met() {
    let said = told(
        r"fn report(of: Build): Files with build.reach {
            return file(names(cast::<List<Reference>>(build.reach!(of.entry), [])));
        }",
        "reach",
    );

    assert_eq!(said, "main helper");
}

#[test]
fn a_tool_runs_a_function_as_a_run_of_its_own() {
    let said = told(
        r#"fn doubled(of: int): int => of * 2;
        fn report(of: Build): Files with build.run {
            match build.run!(comptime tree.reference(doubled), [21]) {
                let ran: Ran => { return file("${cast::<int>(ran.gave, 0)}"); }
                let error: Error => { return file(error.message); }
            }
        }"#,
        "run",
    );

    assert_eq!(said, "42");
}

/// A tool that gives what it fetched under a digest.
fn fetching(digest: &str) -> String {
    format!(
        r#"fn report(of: Build): Files with build.fetch {{
            match build.fetch!("https://example.com/data", "{digest}") {{
                let fetched: bytes => {{ return [{{ path: "told.txt", bytes: fetched }}]; }}
                let error: Error => {{ return file(error.message); }}
            }}
        }}"#
    )
}

/// The SHA-256 of `hello`.
const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

#[test]
fn what_a_tool_fetches_is_held_to_its_digest_and_fetched_once() {
    let remote = Remote {
        held: b"hello".to_vec(),
        asked: RefCell::new(0),
    };
    let store = MemoryStore::new();

    assert_eq!(
        told_from(&fetching(HELLO), "fetch", &store, &remote),
        "hello"
    );
    assert_eq!(
        told_from(&fetching(HELLO), "fetch", &store, &remote),
        "hello"
    );
    assert_eq!(*remote.asked.borrow(), 1);
}

#[test]
fn what_was_fetched_under_another_digest_is_refused() {
    let remote = Remote {
        held: b"other".to_vec(),
        asked: RefCell::new(0),
    };

    let said = told_from(&fetching(HELLO), "fetch", &MemoryStore::new(), &remote);
    assert!(said.contains("its digest is"), "{said}");
}

#[test]
fn a_tool_is_told_the_digest_of_bytes() {
    let said = told(
        r#"fn report(of: Build): Files with build.digest {
            return file(build.digest!("hello".to_bytes()));
        }"#,
        "digest",
    );

    assert_eq!(said, HELLO);
}

#[test]
fn a_tool_keeps_bytes_under_their_digest_and_is_given_them_back() {
    let said = told(
        r#"fn report(of: Build): Files with build.keep, build.kept {
            let back = cast::<bytes>(build.kept!(build.keep!("hello".to_bytes())), null) ?? "lost".to_bytes();
            return [{ path: "told.txt", bytes: back }];
        }"#,
        "keep kept",
    );

    assert_eq!(said, "hello");
}

#[test]
fn a_tool_is_told_the_doc_comment_of_a_definition() {
    let said = told(
        r#"fn report(of: Build): Files with build.doc {
            let doc = fn(of: Reference): string => cast::<string>(build.doc!(of), "none");
            return file("${doc(comptime tree.reference(helper))}, ${doc(of.entry)}");
        }"#,
        "doc",
    );

    assert_eq!(said, "Gives one., none");
}
