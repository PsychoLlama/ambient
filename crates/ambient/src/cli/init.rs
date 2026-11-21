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

/// The library module a new package that is a library begins with.
const LIBRARY: &str = r#"use ambient.test.{check, unmet};

/// What is said to someone.
pub fn greeting(name: string): string => "hello, ${name}";

fn test_a_greeting_names_who_it_greets(): null with unmet {
    check(greeting("you") == "hello, you", "it greets by name");
}
"#;

/// What a new package begins as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begun {
    /// A program: a module with a function to run.
    Program,
    /// A library: the module other packages reach it through.
    Library,
}

impl Begun {
    /// The file it begins with, under `src`, what that holds, and the
    /// command that tries it.
    fn files(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Program => ("main.ab", MAIN, "run"),
            Self::Library => ("lib.ab", LIBRARY, "test"),
        }
    }

    /// The manifest of a package called `name` with the ID `id`. A
    /// program's declares the binary it begins with, so that what is
    /// built of it is called what the package is and not `main`.
    fn manifest(self, name: &str, id: &str) -> String {
        let package = format!("[package]\nname = \"{name}\"\nid = \"{id}\"\nversion = \"0.1.0\"\n");
        match self {
            Self::Program => format!("{package}\n[bin.main]\nname = \"{name}\"\n"),
            Self::Library => package,
        }
    }
}

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
    pub(super) fn init(&self, path: &Path, begun_as: Begun) -> Result<i32, Vec<String>> {
        let (file, text, tries) = begun_as.files();
        let refuse = |why: &str| Err(vec![format!("{}: {why}", path.display())]);
        let mut world = self.world.borrow_mut();
        let placed = placed(world.directory(), path);
        let name = placed.file_name().and_then(|name| name.to_str());
        let Some(name) = name.filter(|name| names(name)) else {
            return refuse("a package is named with letters, digits, and `_`, a letter first");
        };
        let manifest = path.join(MANIFEST);
        let main = path.join("src").join(file);
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
                let text = begun_as.manifest(name, &id);
                world.store(&manifest.to_string_lossy(), text.as_bytes())
            })
            .and_then(|()| {
                if begun {
                    return Ok(());
                }
                world.store(&main.to_string_lossy(), text.as_bytes())
            });
        if let Err(failure) = made {
            return refuse(&failure.message);
        }
        let shown = path.display();
        world.write(&format!(
            "made {shown}: `ambient {tries} {shown}` {tries}s it\n"
        ));
        Ok(0)
    }
}
