//! End-to-end coverage of the structured names index (Phase 6): `store show`
//! resolves types, traits, and abilities (not just objects) with kind, uuid,
//! span, and source path; `store ls --kinds` filters by namespace/kind.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// A package whose one module declares a struct, enum, trait, ability, and a
/// couple of functions.
fn package() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"idx\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("src");
    fs::write(
        src.join("shapes.ab"),
        "pub unique(11111111-1111-1111-1111-111111111111) struct Shape { radius: Number }\n\
         pub unique(22222222-2222-2222-2222-222222222222) enum Color { Red, Green(Number), Blue }\n\
         pub unique(33333333-3333-3333-3333-333333333333) trait Show { fn show(self): String; }\n\
         pub fn area(r: Number): Number { r * r }\n",
    )
    .expect("shapes");
    fs::write(
        src.join("main.ab"),
        "use pkg::shapes::area;\npub fn run(): Number { area(3) }\n",
    )
    .expect("main");
    dir
}

fn run(dir: &Path) {
    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn store(dir: &Path, args: &[&str]) -> Output {
    Command::new(ambient_bin())
        .arg("store")
        .arg("--package")
        .arg(dir)
        .args(args)
        .output()
        .expect("store")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn show_resolves_a_struct_with_kind_uuid_span_and_source() {
    let dir = package();
    run(dir.path());

    let out = store(dir.path(), &["show", "shapes::Shape"]);
    assert!(out.status.success(), "show failed: {}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("(struct)"), "kind: {text}");
    assert!(
        text.contains("uuid: 11111111-1111-1111-1111-111111111111"),
        "uuid: {text}"
    );
    assert!(text.contains("shapes.ab [bytes"), "span+source: {text}");
    assert!(text.contains("{ radius: Number }"), "shape summary: {text}");
}

#[test]
fn show_resolves_enum_and_trait_by_bare_name() {
    let dir = package();
    run(dir.path());

    let color = stdout(&store(dir.path(), &["show", "Color"]));
    assert!(color.contains("(enum)"), "{color}");
    assert!(color.contains("Red | Green(Number) | Blue"), "{color}");

    // `Show` alone is ambiguous (core re-exports its own `Show`); qualify it.
    let show = stdout(&store(dir.path(), &["show", "shapes::Show"]));
    assert!(show.contains("(trait)"), "{show}");
    assert!(
        show.contains("uuid: 33333333-3333-3333-3333-333333333333"),
        "{show}"
    );
}

#[test]
fn show_of_a_function_adds_a_source_location_then_disassembles() {
    let dir = package();
    run(dir.path());

    let out = stdout(&store(dir.path(), &["show", "shapes::area"]));
    assert!(
        out.contains("defined in: shapes.ab [bytes"),
        "location: {out}"
    );
    assert!(out.contains("(plain function)"), "disassembly: {out}");
}

#[test]
fn ls_kinds_filters_by_kind_and_namespace() {
    let dir = package();
    run(dir.path());

    // Precise kind filter: only structs.
    let structs = stdout(&store(dir.path(), &["ls", "--kinds", "struct"]));
    assert!(
        structs.contains("workspace::idx::shapes::Shape"),
        "struct listed: {structs}"
    );
    assert!(!structs.contains("::Color"), "enum excluded: {structs}");
    assert!(!structs.contains("::area"), "fn excluded: {structs}");

    // Namespace filter: all types (struct + enum), no values/traits.
    let types = stdout(&store(dir.path(), &["ls", "--kinds", "type"]));
    assert!(types.contains("::Shape"), "{types}");
    assert!(types.contains("::Color"), "{types}");
    assert!(!types.contains("::area"), "value excluded: {types}");
    assert!(!types.contains("::Show"), "trait excluded: {types}");

    // A kind column is present.
    assert!(types.contains("struct"), "kind column: {types}");
    assert!(types.contains("enum"), "kind column: {types}");
}
