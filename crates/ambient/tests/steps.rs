//! Build steps: a package adds files to builds under a target, which
//! the target's tool is handed. See "Builds" in `spec/tools.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;
use support::scripted::Scripted;

/// A target, and the tool it is built by: a list of what the build
/// steps gave it.
const WEB: &str = r#"
    import ambient.build.{Build, File, Files};
    import ambient.tree.{Reference};

    pub target page built by bundle {}

    pub fn bundle(of: Build): Files {
        let paths = of.files.map(fn(file: File): string => file.path);
        return [{ path: "index.txt", bytes: paths.join_with(",").to_bytes() }];
    }
"#;

/// A package of styles: the key a rule is written under, and the step
/// that gathers every rule a program reaches into one file.
const STYLES: &str = r#"
    import ambient.build;
    import ambient.build.{Build, Annotation, Files};
    import ambient.tree.{Reference};

    pub tag rule;

    pub fn collect(of: Build): Files with build.annotations {
        let found = build.annotations!(of.entry, rule) as List<Annotation> else [];
        let rules = found.map(fn(found: Annotation): string => found.value as string else "");
        return [{ path: "styles.css", bytes: rules.join_with(" ").to_bytes() }];
    }
"#;

const STEP: &str = "grants = [\"ambient.build.annotations\"]\n\
                    [[steps]]\ntarget = \"web.page\"\ntool = \"pkg.collect\"";

/// A workspace of the target's package, the styles' with `step` in its
/// manifest, and an app whose one module is `app`.
fn workspace(step: &str, app: &str) -> BTreeMap<PathBuf, String> {
    let manifest = |name: &str, rest: &str| {
        format!("name = \"{name}\"\nid = \"{name}\"\nversion = \"0.1.0\"\n{rest}")
    };
    let web = "[dependencies]\nweb = { id = \"web\", path = \"../web\" }";
    let both = format!("{web}\nstyles = {{ id = \"styles\", path = \"../styles\" }}");
    let files = [
        ("web/ambient.toml", manifest("web", "")),
        ("web/src/lib.ab", WEB.to_owned()),
        (
            "styles/ambient.toml",
            manifest("styles", &format!("{step}\n{web}")),
        ),
        ("styles/src/lib.ab", STYLES.to_owned()),
        ("app/ambient.toml", manifest("app", &both)),
        ("app/src/main.ab", app.to_owned()),
    ];
    let file = |(path, text): (&str, String)| (PathBuf::from(path), text);
    files.into_iter().map(file).collect()
}

/// Builds `main` of the app under its target `mine`, and gives the
/// world it left.
fn build(files: &BTreeMap<PathBuf, String>) -> Scripted {
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("out")),
        target: Some("main.mine".to_owned()),
    };
    Scripted::default().execute(files, &command).1
}

fn text(world: &Scripted, path: &str) -> String {
    String::from_utf8_lossy(&world.files[path]).into_owned()
}

/// An app that says two rules to the styles, under a target that
/// extends the one the styles' step is for.
const STYLED: &str = r#"
    import web.{page};
    import styles.{rule};

    target mine extends page {}

    @rule("a { color: red }") fn main(): int => more();
    @rule("b { color: blue }") fn more(): int => 1;
"#;

#[test]
fn a_step_adds_files_to_a_build_under_a_target_that_extends_its_own() {
    let built = build(&workspace(STEP, STYLED));

    assert_eq!(built.err, "");
    assert_eq!(
        text(&built, "out/styles.css"),
        "a { color: red } b { color: blue }"
    );
    // The target's tool was handed what the step gave.
    assert_eq!(text(&built, "out/index.txt"), "styles.css");
}

#[test]
fn a_step_runs_for_no_program_that_does_not_reach_its_package() {
    let plain = STYLED
        .replace("@rule(\"a { color: red }\")", "")
        .replace("@rule(\"b { color: blue }\")", "");
    let built = build(&workspace(STEP, &plain));

    assert_eq!(built.err, "");
    assert_eq!(text(&built, "out/index.txt"), "");
    assert!(!built.files.contains_key("out/styles.css"));
}

#[test]
fn a_step_runs_for_no_target_but_its_own() {
    let apart = STYLED
        .replace("import web.{page};", "import web.{bundle};")
        .replace(
            "target mine extends page {}",
            "target mine built by bundle {}",
        );
    let built = build(&workspace(STEP, &apart));

    assert_eq!(built.err, "");
    assert_eq!(text(&built, "out/index.txt"), "");
}

#[test]
fn two_files_for_one_path_are_refused() {
    let twice = format!("{STEP}\n[[steps]]\ntarget = \"web.page\"\ntool = \"pkg.collect\"");
    let built = build(&workspace(&twice, STYLED));

    assert!(
        built.err.contains("two files for `styles.css`"),
        "{}",
        built.err
    );
}

#[test]
fn a_step_that_names_nothing_is_refused() {
    let lost = STEP.replace("pkg.collect", "pkg.missing");
    let built = build(&workspace(&lost, STYLED));

    assert!(
        built.err.contains("the build step `pkg.missing` names"),
        "{}",
        built.err
    );
}
