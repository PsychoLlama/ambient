//! The JavaScript host (`web/host.js`) runs a program built for it:
//! the browser example, as a module, under Node. See "JavaScript
//! hosts" in `spec/targets.md`.
//!
//! The tests themselves are JavaScript (`web/test/`), since what they
//! test is. This builds what they run and runs them, so that the one
//! command that runs every test runs these too.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::Command as Process;

use ambient::cli::Command;
use ambient::project::Disk;
use support::scripted::Scripted;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The ID a manifest says its package has: what a module names the
/// package of each extern by.
fn id(manifest: &str) -> String {
    let said = manifest.lines().find_map(|line| line.strip_prefix("id = "));
    said.unwrap().trim_matches('"').to_owned()
}

#[test]
fn the_javascript_host_runs_a_program_built_for_it() {
    let example = repository().join("examples/browser");
    let command = Command::Build {
        path: example.clone(),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("page.wasm")),
        module: Some("main.page".to_owned()),
    };
    let (code, built) = Scripted::default().execute(&Disk, &command);
    assert_eq!(code, 0, "{}", built.err);

    let module = std::env::temp_dir().join(format!("ambient-page-{}.wasm", std::process::id()));
    std::fs::write(&module, &built.files["page.wasm"]).unwrap();
    let manifest = std::fs::read_to_string(example.join("ambient.toml")).unwrap();
    let ran = Process::new("node")
        .arg("--test")
        .arg(repository().join("web/test/host.test.js"))
        .env("AMBIENT_MODULE", &module)
        .env("AMBIENT_PACKAGE", id(&manifest))
        .output();
    std::fs::remove_file(&module).unwrap();

    let ran = ran.expect("the development shell has `node`");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(ran.status.success(), "{said}");
}
