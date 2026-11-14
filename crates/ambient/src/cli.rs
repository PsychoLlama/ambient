//! The command line: what `ambient` does with a package on disk.
//!
//! Everything it touches is handed to it (files, a store, a world), so a
//! test drives the same code a terminal does.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{Parser, Subcommand};

use crate::backend::native::{Native, Program};
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
    /// Checks every definition of a package and of those it depends on.
    Check {
        /// The package's directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
        let failed = |message: String| vec![message];
        let refused = |error: crate::error::Error| vec![project.report(&error)];
        let root = engine.program(&entry, &host::target()).map_err(refused)?;
        let backend = Native::new().map_err(failed)?;
        let parts = engine.link(&root, &backend).map_err(refused)?;
        let program = Program::load(&parts, &host::natives(&self.world)).map_err(failed)?;
        match program.run(&[]) {
            // A program that gives a number exits with it.
            Ok(Value::Int(code)) => Ok(i32::try_from(code).unwrap_or(FAILED)),
            Ok(_) => Ok(0),
            Err(fault) => {
                let named = Named::reached(&engine, &root);
                let error = program.error();
                let mut lines = vec![match (&error, program.failure()) {
                    (_, Some(native)) => format!("{fault:?}: {native}"),
                    (Some(error), None) => describe(error),
                    (None, None) => format!("{fault:?}"),
                }];
                lines.extend(error.iter().flat_map(|error| named.trace(error)));
                Err(vec![lines.join("\n")])
            }
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
                    problems.insert(project.report(&error));
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

/// What an error value says went wrong: its kind and its message.
fn describe(error: &Value) -> String {
    let Value::Record(fields) = error else {
        return "the program faulted".to_owned();
    };
    let kind = match fields.get("kind") {
        Some(Value::Tag { tag, .. }) => tag.as_str(),
        _ => "fault",
    };
    match fields.get("message") {
        Some(Value::String(message)) if !message.is_empty() => format!("{kind}: {message}"),
        _ => kind.to_owned(),
    }
}

/// The definitions a program reaches, by hash: what turns a trace's
/// frames back into names.
struct Named(BTreeMap<String, Symbol>);

impl Named {
    fn reached(engine: &Engine, root: &Symbol) -> Self {
        let mut names = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![root.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            if let Ok(hash) = engine.hash(&symbol) {
                names.insert(hash.to_string(), symbol.clone());
            }
            if let Ok(open) = engine.check(&symbol) {
                pending.extend(open.slots.iter().cloned());
            }
        }
        Self(names)
    }

    /// A line for each frame of an error's trace, nearest first.
    fn trace(&self, error: &Value) -> Vec<String> {
        let Value::Record(fields) = error else {
            return Vec::new();
        };
        let Some(Value::List(frames)) = fields.get("trace") else {
            return Vec::new();
        };
        let line = |frame: &Value| {
            let Value::Record(frame) = frame else {
                return None;
            };
            let Some(Value::String(hash)) = frame.get("definition") else {
                return None;
            };
            let name = self.0.get(hash).map_or(hash.clone(), Symbol::to_string);
            Some(format!("  in {name}"))
        };
        frames.iter().filter_map(line).collect()
    }
}
