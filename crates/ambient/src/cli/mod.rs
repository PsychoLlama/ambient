//! The command line: what `ambient` does with a package on disk.
//!
//! Everything it touches is handed to it (files, a store, a world), so a
//! test drives the same code a terminal does.

mod build;
mod building;
mod files;
mod init;
mod launch;
mod linking;
mod machine;
mod package;
mod test;
mod trace;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{Parser, Subcommand};

pub use ambient_host::launch::carried;
use ambient_host::launch::{FAILED, code, started};
pub use build::{LAUNCHER, start};
pub use building::engine;
use launch::Launch;
pub use linking::{ARCHIVE, Link, Linker, Unlinked, symbol};
pub use machine::Machine;

use crate::engine::Engine;
use crate::error::Error;
use crate::host::{self, World};
use crate::project::{self, Files, Project, Snapshots, Update};
use crate::store::Store;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, PackageId, Symbol};
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
        /// Runs the program under this target of the package, a
        /// module path and a name, which extends the command-line
        /// target: what binds a native handler of a package.
        #[arg(long)]
        target: Option<String>,
        /// What the program is started with.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Makes a package depend on another: writes the dependency into
    /// its manifest.
    Add {
        /// The directory of the package to depend on.
        on: PathBuf,
        /// The name the package uses for it; its own, if not said.
        #[arg(long = "as")]
        name: Option<String>,
        /// The directory of the package that is to depend on it.
        #[arg(long = "in", default_value = ".")]
        path: PathBuf,
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
        /// Builds the program under this target of the package, a
        /// module path and a name: the files the tool the target is
        /// built by gives, or with no tool the WebAssembly module
        /// alone. The output is then a directory.
        #[arg(long)]
        target: Option<String>,
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
    /// Reads again the branch or tag a dependency is on, and locks the
    /// commit it is at now; with none named, each one in the workspace.
    Update {
        /// The dependency, by the name the package uses for it.
        name: Option<String>,
        /// The directory of the package that depends on it.
        #[arg(long = "in", default_value = ".")]
        path: PathBuf,
    },
    /// Copies a dependency into the workspace, where it can be read
    /// and changed; with none named, says which copies were changed.
    Vendor {
        /// The dependency, by the name the package uses for it.
        name: Option<String>,
        /// The directory of the package that depends on it.
        #[arg(long = "in", default_value = ".")]
        path: PathBuf,
    },
    /// Says a package's snapshot: the hash of the files that are it.
    Snapshot {
        /// The package's directory.
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

/// What running says of a target the command line cannot be the
/// world of.
const UNRUN: &str = "a program is run under the command-line target, or one that extends it";

/// What a command works with.
pub struct Session<'a> {
    pub files: &'a dyn Files,
    pub store: &'a dyn Store,
    /// The snapshots this machine keeps, and what fetches one.
    pub snapshots: &'a dyn Snapshots,
    pub world: Rc<RefCell<dyn World>>,
    /// The file of the command itself, which loads what it builds; a
    /// session that has none builds nothing.
    pub launcher: Option<PathBuf>,
    /// What joins the native bundles a program reaches with what
    /// loads it.
    pub linker: &'a dyn Linker,
}

impl Session<'_> {
    /// Does what `command` says, and gives the code to exit with. What
    /// went wrong is written to the world's standard error.
    #[must_use]
    pub fn execute(&self, command: &Command) -> i32 {
        let done = match command {
            Command::Add { on, name, path } => self.add(path, on, name.as_deref()),
            Command::Run {
                path,
                entry,
                target,
                ..
            } => self.run(path, entry, target.as_deref()),
            Command::Build {
                path,
                entry,
                output,
                target: None,
            } => self.build(path, entry, None, output.as_deref()),
            Command::Build {
                path,
                entry,
                output,
                target: Some(target),
            } => self.build(path, entry, Some(target), output.as_deref()),
            Command::Check { path } => self.check(path).map(|()| 0),
            Command::Init { path } => self.init(path),
            Command::Update { name, path } => self.update(path, name.as_deref()),
            Command::Vendor { name, path } => self.vendor(path, name.as_deref()),
            Command::Snapshot { path } => self.snapshot(path),
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
        self.open_with(path, &Update::Nothing)
    }

    fn open_with(&self, path: &Path, update: &Update) -> Result<Project, Vec<String>> {
        let project = project::open_with(self.files, self.snapshots, path, update);
        let project = project.map_err(|error| vec![error.to_string()])?;
        // What opening locked is what every later build is held to.
        if let Some((lock, text)) = &project.locked {
            let stored = self
                .world
                .borrow_mut()
                .store(&lock.to_string_lossy(), text.as_bytes());
            stored.map_err(|failure| vec![format!("{}: {}", lock.display(), failure.message)])?;
        }
        Ok(project)
    }

    /// The target a program of `project` is run or built under on a
    /// command line: the one `named` in its package, or the
    /// command-line target itself. Nothing, if the one named does not
    /// extend that one: such a target is built by a tool of its own.
    fn target(
        project: &Project,
        engine: &Engine,
        named: Option<&str>,
    ) -> Result<Option<Symbol>, Vec<String>> {
        let standard = crate::system::target();
        let Some(named) = named else {
            return Ok(Some(standard));
        };
        let target = entry_of(project, named);
        let extends = engine.extends(&target, &standard);
        let extends = extends.map_err(|error| vec![report(project, engine, &error)])?;
        Ok(extends.then_some(target))
    }

    fn run(&self, path: &Path, entry: &str, target: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let Some(target) = Self::target(&project, &engine, target)? else {
            return Err(vec![UNRUN.to_owned()]);
        };
        let linked = launch.link(&entry_of(&project, entry), &target)?;
        let said = |problem: String| vec![problem];
        // A program that reaches a native bundle is run by what was
        // linked with it, which this process was not.
        if let Some(link) = self.linking(&project, &linked.parts).map_err(said)? {
            let executable = self.executable(&launch, &linked, Some(&link))?;
            let args = self.world.borrow().args();
            return self.linker.run(&executable, &args).map_err(said);
        }
        let natives = host::natives(&self.world, linked.trees.clone());
        let gave = started(&linked.parts, &natives, || launch.frames(&linked.root))?;
        Ok(code(&gave))
    }

    /// The packages a command at `path` covers: every member, at a
    /// workspace's directory.
    fn covered(&self, path: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
        project::covered(self.files, path).map_err(|error| vec![error.to_string()])
    }

    /// Checks each package at `path`, and reports what is wrong with
    /// any of them.
    fn check(&self, path: &Path) -> Result<(), Vec<String>> {
        let mut problems = BTreeSet::new();
        for package in self.covered(path)? {
            problems.extend(self.checked(&package).err().into_iter().flatten());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.into_iter().collect())
        }
    }

    fn checked(&self, path: &Path) -> Result<(), Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let mut problems = BTreeSet::new();
        for read in &project.modules {
            let Some(module) = project.workspace.module(&read.module) else {
                continue;
            };
            let mut report = |error: Error| problems.insert(report(&project, &engine, &error));
            for definition in module.definitions() {
                let namespace = definition.kind.namespace();
                let symbol = read.module.symbol(namespace, &definition.name);
                let checked = match definition.kind {
                    DefinitionKind::Function(_) | DefinitionKind::Generated(_) => {
                        engine.check(&symbol).map(|_| ())
                    }
                    // A generated module is checked as what its call
                    // gave, below.
                    DefinitionKind::Module(_) => Ok(()),
                    _ => engine.interface(&symbol).map(|_| ()),
                };
                checked.map_err(&mut report).ok();
            }
            engine.imports(&read.module).map_err(&mut report).ok();
            let made = engine.generated_in(&read.module).map_err(&mut report);
            for symbol in made.unwrap_or_default() {
                engine.check(&symbol).map_err(&mut report).ok();
            }
        }
        // A lint is run over what checks: it asks the build about
        // definitions, and is told nothing of one that is wrong.
        if problems.is_empty() {
            let root = PackageId(project.root.clone());
            let found = engine.linted(&root, "").unwrap_or_else(|error| vec![error]);
            problems.extend(found.iter().map(|error| report(&project, &engine, error)));
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

/// What is wrong, where it is in the text, and the code it is in if
/// that is code a comptime call gave, which the text does not have.
fn report(project: &Project, engine: &Engine, error: &Error) -> String {
    let mut report = project.report(error, engine.blame(error));
    if let Some(given) = engine.shown(error) {
        report.push_str("\nin the code the call gave:\n");
        report.push_str(given.trim_end());
    }
    report
}
