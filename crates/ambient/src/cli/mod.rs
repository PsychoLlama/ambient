//! The command line: what `ambient` does with a package on disk.
//!
//! Everything it touches is handed to it (files, a store, a world), so a
//! test drives the same code a terminal does.

mod build;
mod init;
mod launch;
mod test;
mod trace;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{Parser, Subcommand};

pub use build::{carried, start};
use launch::Launch;

use crate::engine::Engine;
use crate::host::{self, World};
use crate::project::{self, Files, Project};
use crate::store::Store;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;
use crate::workspace::Sources;

/// The Ambient programming language.
#[derive(Debug, Parser)]
#[command(name = "ambient", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Runs a function of a package as a program.
    Run {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// The function to run: a module path and a name.
        #[arg(long, default_value = "main.main")]
        entry: String,
        /// What the program is started with.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Makes an executable of a function of a package: one file that
    /// runs it without the package or the compiler.
    Build {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// The function to run: a module path and a name.
        #[arg(long, default_value = "main.main")]
        entry: String,
        /// The file to make; one named for the package, if not said.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Checks every definition of a package and of those it depends on.
    Check {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Makes a package to begin from: one that runs and has a test.
    Init {
        /// The directory to make it in, whose name the package takes.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Runs every test of a package: each function that takes nothing
    /// and whose name begins with `test_`.
    Test {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Runs only the tests whose module path and name hold this.
        #[arg(long)]
        only: Option<String>,
    },
}

/// What a command works with.
pub struct Session<'a> {
    pub files: &'a dyn Files,
    pub store: &'a dyn Store,
    pub world: Rc<RefCell<dyn World>>,
    /// The file of the command itself, which loads what it builds; a
    /// session that has none builds nothing.
    pub launcher: Option<PathBuf>,
}

/// The code a command that could not do its work exits with.
const FAILED: i32 = 1;

impl Session<'_> {
    /// Does what `command` says, and gives the code to exit with. What
    /// went wrong is written to the world's standard error.
    #[must_use]
    pub fn execute(&self, command: &Command) -> i32 {
        let done = match command {
            Command::Run { path, entry, .. } => self.run(path, entry),
            Command::Build {
                path,
                entry,
                output,
            } => self.build(path, entry, output.as_deref()),
            Command::Check { path } => self.check(path).map(|()| 0),
            Command::Init { path } => self.init(path),
            Command::Test { path, only } => self.test(path, only.as_deref()),
        };
        done.unwrap_or_else(|problems| {
            let mut world = self.world.borrow_mut();
            for problem in problems {
                world.warn(&format!("error: {problem}\n"));
            }
            FAILED
        })
    }

    fn open(&self, path: &Path) -> Result<Project, Vec<String>> {
        project::open(self.files, path).map_err(|error| vec![error.to_string()])
    }

    fn run(&self, path: &Path, entry: &str) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = Engine::new(&project.workspace, self.store);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let gave = launch.run(
            &entry_of(&project, entry),
            &crate::system::target(),
            &host::natives(&self.world),
        )?;
        Ok(code(&gave))
    }

    fn check(&self, path: &Path) -> Result<(), Vec<String>> {
        let project = self.open(path)?;
        let engine = Engine::new(&project.workspace, self.store);
        let mut problems = BTreeSet::new();
        for read in &project.modules {
            let Some(module) = project.workspace.module(&read.module) else {
                continue;
            };
            for definition in module.definitions() {
                let namespace = definition.kind.namespace();
                let symbol = read.module.symbol(namespace, &definition.name);
                let checked = match definition.kind {
                    DefinitionKind::Function(_) => engine.check(&symbol).map(|_| ()),
                    _ => engine.interface(&symbol).map(|_| ()),
                };
                if let Err(error) = checked {
                    problems.insert(project.report(&error, engine.blame(&error)));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.into_iter().collect())
        }
    }
}

/// The function of a project's package that `entry` names: a module
/// path and a name.
fn entry_of(project: &Project, entry: &str) -> Symbol {
    let (module, name) = entry.rsplit_once('.').unwrap_or(("", entry));
    ModuleId::new(&project.root, module).value(name)
}

/// What a program that gave `gave` exits with: the number, if it gave
/// one.
fn code(gave: &Value) -> i32 {
    match gave {
        Value::Int(code) => i32::try_from(*code).unwrap_or(FAILED),
        _ => 0,
    }
}
