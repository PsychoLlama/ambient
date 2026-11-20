//! `ambient init`: the files of a package that runs and has a test, to
//! begin one from. See `docs/spec/packages.md`.

use std::path::{Component, Path, PathBuf};

use super::Session;
use crate::host::{Failure, World};
use crate::project::MANIFEST;

/// The module a new package begins with.
const MAIN: &str = r#"use system.console;
use ambient.test.{check, unmet};

/// What the program says to someone.
fn greeting(name: string): string => "hello, ${name}";

pub fn main(): null with console.print {
    console.print!(greeting("world"));
}

fn test_a_greeting_names_who_it_greets(): null with unmet {
    check(greeting("you") == "hello, you", "it greets by name");
}
"#;

/// Whether other packages could import a package by this name.
pub(super) fn names(name: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    name.starts_with(|c: char| c.is_ascii_alphabetic()) && name.chars().all(word)
}

/// The path `path` is from `directory`, with no step that goes nowhere
/// or back: what `.` and `..` name is in the directory, not the path.
fn placed(directory: Option<String>, path: &Path) -> PathBuf {
    let from = directory.map(PathBuf::from).unwrap_or_default();
    from.join(path)
        .components()
        .fold(PathBuf::new(), |mut placed, step| {
            match step {
                Component::CurDir => {}
                Component::ParentDir => drop(placed.pop()),
                step => placed.push(step),
            }
            placed
        })
}

/// An ID for a new package: 128 bits no other package was given.
fn identifier(world: &mut dyn World) -> Result<String, Failure> {
    Ok(format!("{:016x}{:016x}", world.random()?, world.random()?))
}

impl Session<'_> {
    /// Makes a package in the directory at `path`, named for it.
    pub(super) fn init(&self, path: &Path) -> Result<i32, Vec<String>> {
        let refuse = |why: &str| Err(vec![format!("{}: {why}", path.display())]);
        let mut world = self.world.borrow_mut();
        let placed = placed(world.directory(), path);
        let name = placed.file_name().and_then(|name| name.to_str());
        let Some(name) = name.filter(|name| names(name)) else {
            return refuse("a package is named with letters, digits, and `_`, a letter first");
        };
        let manifest = path.join(MANIFEST);
        let main = path.join("src").join("main.ab");
        if world.exists(&manifest.to_string_lossy()) {
            return refuse("there is a package here already");
        }
        // What is here already is someone's: only what is missing is made.
        let begun = world.exists(&main.to_string_lossy());
        // The directories first: the package's own may not be there yet.
        let made = world
            .make_directory(&path.join("src").to_string_lossy())
            .and_then(|()| identifier(&mut *world))
            .and_then(|id| {
                let text = format!("name = \"{name}\"\nid = \"{id}\"\nversion = \"0.1.0\"\n");
                world.store(&manifest.to_string_lossy(), text.as_bytes())
            })
            .and_then(|()| {
                if begun {
                    return Ok(());
                }
                world.store(&main.to_string_lossy(), MAIN.as_bytes())
            });
        if let Err(failure) = made {
            return refuse(&failure.message);
        }
        let shown = path.display();
        world.write(&format!("made {shown}: `ambient run {shown}` runs it\n"));
        Ok(0)
    }
}
