//! `ambient new`: the files of a package that runs and has a test, to
//! begin one from. See `spec/packages.md`.

use std::path::Path;

use super::Session;
use crate::project::MANIFEST;

/// The module a new package begins with.
const MAIN: &str = r#"import ambient.console;
import ambient.test.{check};

/// What the program says to someone.
fn greeting(name: string): string => "hello, ${name}";

pub fn main(): null with console.print {
    console.print!(greeting("world"));
}

fn test_a_greeting_names_who_it_greets(): null with check {
    check!(greeting("you") == "hello, you", "it greets by name");
}
"#;

/// Whether other packages could import a package by this name.
fn names(name: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    name.starts_with(|c: char| c.is_ascii_alphabetic()) && name.chars().all(word)
}

impl Session<'_> {
    /// Makes a package in the directory at `path`, named for it.
    pub(super) fn new_package(&self, path: &Path) -> Result<i32, Vec<String>> {
        let refuse = |why: &str| Err(vec![format!("{}: {why}", path.display())]);
        let name = path.file_name().and_then(|name| name.to_str());
        let Some(name) = name.filter(|name| names(name)) else {
            return refuse("a package is named with letters, digits, and `_`, a letter first");
        };
        let manifest = path.join(MANIFEST);
        let main = path.join("src").join("main.ab");
        let mut world = self.world.borrow_mut();
        if world.exists(&manifest.to_string_lossy()) {
            return refuse("there is a package here already");
        }
        let made = world
            .make_directory(&path.join("src").to_string_lossy())
            .and_then(|()| {
                world.store(&manifest.to_string_lossy(), &format!("name = \"{name}\"\n"))
            })
            .and_then(|()| world.store(&main.to_string_lossy(), MAIN));
        if let Err(failure) = made {
            return refuse(&failure.message);
        }
        let shown = path.display();
        world.write(&format!("made {shown}: `ambient run {shown}` runs it\n"));
        Ok(0)
    }
}
