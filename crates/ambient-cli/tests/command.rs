//! The command, run as a process: what it builds runs with the package
//! it was built from gone. See "Building" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

use std::process::Command;

const GREETS: &str = r#"
    use system.console;
    use system.env;

    fn main(): int with console.print, env.args {
        console.print!("hello, " + env.args!().join_with(" and "));
        return 3;
    }
"#;

/// The one test that touches the disk through the command itself.
#[test]
fn the_command_builds_a_file_that_runs_on_its_own() {
    let ambient = env!("CARGO_BIN_EXE_ambient");
    let directory = std::env::temp_dir().join(format!("ambient-build-{}", std::process::id()));
    let package = directory.join("app");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        package.join("ambient.toml"),
        "[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"",
    )
    .unwrap();
    std::fs::write(package.join("src/main.ab"), GREETS).unwrap();
    let executable = directory.join("greet");

    let build = Command::new(ambient)
        .args(["build", "--output", "../greet"])
        .current_dir(&package)
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::remove_dir_all(&package).unwrap();
    let ran = Command::new(&executable)
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
