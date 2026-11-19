//! Annotations: what a definition says to a build, which a tool
//! collects and a program never reads. See `spec/annotations.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::builder::{annotated, function, int, string, tag, ty};
use ambient::cli::Command;
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use support::scripted::Scripted;
use support::{program, value};

/// A tool that gives one file: the notes its entry reaches, in the
/// order the build gives them.
const TOOL: &str = r#"
    use ambient.build;
    use ambient.code;
    use ambient.tree;
    use ambient.build.{Build, Annotation, Files};
    use ambient.tree.{Reference};
    use ambient.Error;

    tag note;
    tag other;

    target page built by notes {}

    fn line(found: Annotation): string {
        match found.value {
            let text: string => { return "${found.on.name}=${text}"; }
            let tagged: other => { return "${found.on.name}=other"; }
            let named: Reference => { return "${found.on.name}=&${named.name}"; }
            _ => { return "${found.on.name}=?"; }
        }
    }

    fn notes(of: Build): Files with build.annotations {
        match build.annotations!(of.entry, note) {
            let error: Error => { return [{ path: "notes.txt", bytes: error.message.to_bytes() }]; }
            let found: List<Annotation> => {
                return [{ path: "notes.txt", bytes: found.map(line).join_with(" ").to_bytes() }];
            }
        }
    }
"#;

/// Builds `main` of a package whose module is [`TOOL`] and then
/// `program`, and gives the world it left.
fn build(program: &str) -> Scripted {
    let manifest = "name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n\
                    grants = [\"ambient.build.annotations\"]";
    let files: BTreeMap<PathBuf, String> = [
        ("app/ambient.toml", manifest.to_owned()),
        ("app/src/main.ab", format!("{TOOL}{program}")),
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
    Scripted::default().execute(&files, &command).1
}

fn notes(built: &Scripted) -> String {
    assert_eq!(built.err, "");
    String::from_utf8_lossy(&built.files["out/notes.txt"]).into_owned()
}

#[test]
fn a_tool_is_given_the_annotations_reached_in_the_order_they_are_met() {
    let built = build(
        r#"
        #[note("entry")] fn main(): int => later() + sooner();
        #[note("b")] #[other("unasked")] fn sooner(): int => shared();
        #[note("a")] fn later(): int => shared();
        #[note("both")] fn shared(): int => 1;
        #[note("unreached")] fn apart(): int => 2;
        "#,
    );

    assert_eq!(notes(&built), "main=entry later=a shared=both sooner=b");
}

#[test]
fn what_a_comptime_call_says_in_the_code_it_gives_is_met_where_the_code_is() {
    let built = build(
        r#"
        fn marked(): tree.Expr =>
            code.build(code.annotated(comptime tree.reference(note), "inside", code.literal(1)));

        fn main(): int => later() + (comptime marked()) + sooner();
        #[note("b")] fn sooner(): int => 1;
        #[note("a")] fn later(): int => 1;
        "#,
    );

    assert_eq!(notes(&built), "later=a main=inside sooner=b");
}

#[test]
fn an_extern_and_an_ability_are_annotated_though_neither_has_a_body() {
    let built = build(
        r#"
        #[note("asks")] ability say(text: string): null;
        #[note("answers")] extern handler said for say;
        fn main(): null with say => say!("hello");
        "#,
    );

    assert_eq!(notes(&built), "say=asks");
}

#[test]
fn a_value_names_a_tag_or_refers_to_a_definition() {
    let built = build(
        r"
        #[note(other)] fn main(): int => helper();
        #[note(main)] fn helper(): int => 1;
        ",
    );

    assert_eq!(notes(&built), "main=other helper=&main");
}

#[test]
fn a_key_that_is_no_tag_is_refused() {
    let built = build("type Plain = int;\n#[Plain(1)] fn main(): int => 1;");

    assert!(notes(&built).ends_with("the key of an annotation is a tag"));
}

#[test]
fn a_value_that_is_not_plain_data_is_refused() {
    let built = build("#[note(1 + 1)] fn main(): int => 1;");

    assert!(notes(&built).contains("the value of an annotation is plain data"));
}

/// A program whose entry is annotated with `said`, if with anything.
fn noted(said: Option<&str>) -> Vec<Definition> {
    let main = function("main").returns(ty::int()).body(int(1));
    let main = said
        .into_iter()
        .fold(main, |main, said| main.annotated("note", string(said)));
    vec![tag("note"), main]
}

#[test]
fn an_annotation_is_content_of_its_definition() {
    let hash = |said| {
        let workspace = program(noted(said));
        let store = MemoryStore::new();
        Engine::new(&workspace, &store)
            .hash(&value("main", "main"))
            .unwrap()
    };

    assert_ne!(hash(Some("one")), hash(Some("two")));
    assert_ne!(hash(Some("one")), hash(None));
    assert_eq!(hash(Some("one")), hash(Some("one")));
}

#[test]
fn an_annotation_on_an_expression_is_content_and_nothing_to_a_program() {
    let said = |said| {
        let body = annotated("note", string(said), int(1));
        program([tag("note"), function("main").returns(ty::int()).body(body)])
    };
    let hash = |workspace: &ambient::workspace::Workspace| {
        let store = MemoryStore::new();
        Engine::new(workspace, &store)
            .hash(&value("main", "main"))
            .unwrap()
    };

    assert_ne!(hash(&said("one")), hash(&said("two")));
    assert_eq!(
        support::run_in(&said("one")),
        Ok(ambient::value::Value::Int(1))
    );
}

#[test]
fn a_program_runs_as_it_would_with_no_annotation() {
    let workspace = program(noted(Some("one")));

    assert_eq!(
        support::run_in(&workspace),
        Ok(ambient::value::Value::Int(1))
    );
}
