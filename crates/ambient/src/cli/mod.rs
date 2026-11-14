//! The command line: what `ambient` does with a package on disk.
//!
//! Everything it touches is handed to it (files, a store, a world), so a
//! test drives the same code a terminal does.

mod new;
mod test;
mod trace;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;
use clap::{Parser, Subcommand};

use crate::backend::native::{Native, Natives, Program};
use crate::engine::Engine;
use crate::host::{self, World};
use crate::project::{self, Files, Project};
use crate::store::Store;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;
use crate::workspace::Sources;
use trace::{Named, describe};

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
    /// Checks every definition of a package and of those it depends on.
    Check {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Makes a package to begin from: one that runs and has a test.
    New {
        /// The directory to make it in, whose name the package takes.
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
            Command::Check { path } => self.check(path).map(|()| 0),
            Command::New { path } => self.new_package(path),
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
        let (module, name) = entry.rsplit_once('.').unwrap_or(("", entry));
        let entry = ModuleId::new(&project.root, module).value(name);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        match launch.run(&entry, &host::target(), &host::natives(&self.world))? {
            // A program that gives a number exits with it.
            Value::Int(code) => Ok(i32::try_from(code).unwrap_or(FAILED)),
            _ => Ok(0),
        }
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

/// A package, opened, and the engine that builds it.
struct Launch<'a> {
    project: &'a Project,
    engine: &'a Engine<'a>,
}

impl Launch<'_> {
    /// Runs `entry` as a program under `target`, with `natives` for
    /// what the target binds. Gives what it gave, or says why it did
    /// not get that far: what is wrong with it, or the fault that
    /// ended it and the functions the fault left.
    fn run(
        &self,
        entry: &Symbol,
        target: &Symbol,
        natives: &Natives,
    ) -> Result<Value, Vec<String>> {
        let Self { project, engine } = self;
        let failed = |message: String| vec![message];
        let refused =
            |error: crate::error::Error| vec![project.report(&error, engine.blame(&error))];
        let root = engine.program(entry, target).map_err(refused)?;
        let backend = Native::new().map_err(failed)?;
        let parts = engine.link(&root, &backend).map_err(refused)?;
        // An error nothing dealt with reaches the root, which keeps it:
        // the program ends there, and is reported as one that faulted.
        let thrown = Rc::new(RefCell::new(None));
        let kept = Rc::clone(&thrown);
        let natives = natives
            .clone()
            .with(FAULT_PACKAGE, "system.thrown", move |args| {
                *kept.borrow_mut() = args.first().cloned();
                Ok(Value::Null)
            });
        let program = Program::load(&parts, &natives).map_err(failed)?;
        let traced = |said: String, error: Option<&Value>| {
            let named = Named::reached(project, engine, &root);
            let mut lines = vec![said];
            lines.extend(error.iter().flat_map(|error| named.trace(error)));
            vec![lines.join("\n")]
        };
        let gave = program.run(&[]).map_err(|fault| {
            let error = program.error();
            let said = match (&error, program.failure()) {
                (_, Some(native)) => format!("{fault:?}: {native}"),
                (Some(error), None) => describe(error),
                (None, None) => format!("{fault:?}"),
            };
            traced(said, error.as_ref())
        })?;
        match thrown.take() {
            Some(error) => Err(traced(describe(&error), Some(&error))),
            None => Ok(gave),
        }
    }
}
