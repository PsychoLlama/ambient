//! The command line: what `ambient` does with a package on disk.
//!
//! Everything it touches is handed to it (files, a store, a world), so a
//! test drives the same code a terminal does.

mod build;
mod building;
mod chosen;
mod files;
mod header;
mod init;
mod launch;
mod linking;
mod machine;
mod package;
mod picked;
mod sealing;
mod test;
mod trace;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{Parser, Subcommand};

pub use ambient_host::launch::carried;
use ambient_host::launch::{FAILED, code, started};
pub use build::start;
pub use building::engine;
pub use chosen::Chosen;
pub use header::header;
use init::Begun;
use launch::Launch;
pub use linking::{Link, Linker, Unlinked, archive, built, symbol};
pub use machine::Machine;
use picked::Picked;

use crate::engine::Engine;
use crate::error::Error;
use crate::host::{self, World};
use crate::project::{self, Binary, Files, Project, Snapshots, Update};
use crate::store::Store;
use crate::surface::DefinitionKind;
use crate::symbol::{PackageId, Symbol};
use crate::workspace::Sources;

/// The Ambient programming language.
#[derive(Debug, Parser)]
#[command(name = "ambient", version)]
pub struct Cli {
    /// Fetches nothing: a build fails where it would have to.
    #[arg(long, global = true)]
    pub offline: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Runs a binary of a package as a program.
    Run {
        #[command(flatten)]
        at: Chosen,
        /// The binary to run, by the name its manifest declares it
        /// under: the only one there is, or `main`, if not said. With
        /// no name, says what there is.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
        bin: Option<String>,
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
        #[command(flatten)]
        at: Chosen,
    },
    /// Makes an executable of each binary of a package: one file that
    /// runs it without the package or the compiler. A binary whose
    /// target is built by a tool gives that tool's files instead.
    Build {
        #[command(flatten)]
        at: Chosen,
        /// The one binary to build, by the name its manifest declares
        /// it under; every one, if not said. With no name, says what
        /// there is.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
        bin: Option<String>,
        /// The file to make of the one binary built, or the directory
        /// for a tool's files; one named as the binary says, if not
        /// said.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Checks every definition of a package and of those it depends on.
    Check {
        #[command(flatten)]
        at: Chosen,
    },
    /// Fetches what a build of a package reads, for building later
    /// with no network: every package the workspace's lock holds, and
    /// the native bundles for this machine that its binaries reach.
    Fetch {
        #[command(flatten)]
        at: Chosen,
        /// The one binary a build will be of; every one, if not said.
        /// With no name, says what there is.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
        bin: Option<String>,
    },
    /// Says the C header a native bundle of a package is written
    /// against: what it may call, and a prototype for each extern.
    Header {
        #[command(flatten)]
        at: Chosen,
    },
    /// Makes a package to begin from: one that runs and has a test.
    Init {
        /// The directory to make it in, whose name the package takes.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Makes a program, with a binary to run: what is made when
        /// neither is said.
        #[arg(long, conflicts_with = "lib")]
        bin: bool,
        /// Makes a library, for other packages to depend on, with a
        /// test and nothing to run.
        #[arg(long)]
        lib: bool,
    },
    /// Reads again the branch or tag a dependency is on, and locks the
    /// commit it is at now; with none named, each one in the workspace.
    Update {
        /// The dependency, by the name the package uses for it.
        name: Option<String>,
        #[command(flatten)]
        at: Chosen,
    },
    /// Copies a dependency into the workspace, where it can be read
    /// and changed; with none named, says which copies were changed.
    Vendor {
        /// The dependency, by the name the package uses for it.
        name: Option<String>,
        #[command(flatten)]
        at: Chosen,
    },
    /// Says a package's snapshot: the hash of the files that are it.
    Snapshot {
        #[command(flatten)]
        at: Chosen,
    },
    /// Runs every test of a package: each function that takes nothing
    /// and whose name begins with `test_`.
    Test {
        #[command(flatten)]
        at: Chosen,
        /// Runs only the tests whose module path and name hold this.
        #[arg(long)]
        only: Option<String>,
    },
}

/// What running says of a target the command line cannot be the
/// world of.
const UNRUN: &str = "a program is run under the command-line target, or one that extends it: \
     this binary's target is built by a tool, so build it";

/// What a command works with.
pub struct Session<'a> {
    /// Where the command is run: what it is for is the package or the
    /// workspace there or above.
    pub directory: &'a Path,
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
            Command::Add { on, name, at } => self
                .one(at)
                .and_then(|path| self.add(&path, on, name.as_deref())),
            Command::Run { at, bin, .. } => self.run(at, bin.as_deref()),
            Command::Build { at, bin, output } => self.build(at, bin.as_deref(), output.as_deref()),
            Command::Check { at } => self.check(at).map(|()| 0),
            Command::Init { path, lib, .. } => {
                let begun = if *lib { Begun::Library } else { Begun::Program };
                self.init(path, begun)
            }
            Command::Update { name, at } => self
                .one(at)
                .and_then(|path| self.update(&path, name.as_deref())),
            Command::Vendor { name, at } => self
                .one(at)
                .and_then(|path| self.vendor(&path, name.as_deref())),
            Command::Fetch { at, bin } => self.fetch(at, bin.as_deref()),
            Command::Header { at } => self.one(at).and_then(|path| self.header(&path)),
            Command::Snapshot { at } => self.one(at).and_then(|path| self.snapshot(&path)),
            Command::Test { at, only } => self.test(at, only.as_deref()),
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

    /// The target `binary` is run or built under on a command line:
    /// the one it names, or the command-line target itself. Nothing,
    /// if the one it names does not extend that one: such a target is
    /// built by a tool of its own.
    fn target(
        project: &Project,
        engine: &Engine,
        binary: &Binary,
    ) -> Result<Option<Symbol>, Vec<String>> {
        let standard = crate::system::target();
        let Some(target) = binary.target.clone() else {
            return Ok(Some(standard));
        };
        let extends = engine.extends(&target, &standard);
        let extends = extends.map_err(|error| vec![report(project, engine, &error)])?;
        Ok(extends.then_some(target))
    }

    /// Fetches what a build of the packages `chosen` is reads: opening
    /// each fetches the packages, and asking what is to be linked
    /// fetches the native bundles, for the binary `bin` or else for
    /// each there is.
    fn fetch(&self, chosen: &Chosen, bin: Option<&str>) -> Result<i32, Vec<String>> {
        let offered = self.offered(chosen)?;
        // A package with nothing to run has nothing to link.
        let none = bin.is_none()
            && offered
                .iter()
                .all(|package| package.project.bins.is_empty());
        let picked = if none {
            Vec::new()
        } else {
            picked::every(&offered, bin)?
        };
        for Picked { package, binary } in picked {
            let project = &package.project;
            let engine = self.engine(project);
            let launch = Launch {
                project,
                engine: &engine,
            };
            // What a tool builds is linked with no native bundle.
            let Some(target) = Self::target(project, &engine, binary)? else {
                continue;
            };
            let linked = launch.link(&binary.entry, &target)?;
            let reached = self.linking(project, &linked.parts);
            reached.map_err(|problem| vec![problem])?;
        }
        Ok(0)
    }

    fn run(&self, chosen: &Chosen, bin: Option<&str>) -> Result<i32, Vec<String>> {
        let offered = self.offered(chosen)?;
        let Picked { package, binary } = picked::one(&offered, bin)?;
        let project = &package.project;
        let engine = self.engine(project);
        let launch = Launch {
            project,
            engine: &engine,
        };
        let Some(target) = Self::target(project, &engine, binary)? else {
            return Err(vec![UNRUN.to_owned()]);
        };
        let linked = launch.link(&binary.entry, &target)?;
        let said = |problem: String| vec![problem];
        // A program that reaches a native bundle is run by what was
        // linked with it, which this process was not.
        let link = self.linking(project, &linked.parts).map_err(said)?;
        if !link.bundles.is_empty() {
            let executable = self.executable(&launch, &linked, &link)?;
            let args = self.world.borrow().args();
            return self.linker.run(&executable, &args).map_err(said);
        }
        let natives = host::natives(&self.world, linked.trees.clone());
        let gave = started(&linked.parts, &natives, || launch.frames(&linked.root))?;
        Ok(code(&gave))
    }

    /// Checks each package a command is for, and reports what is
    /// wrong with any of them.
    fn check(&self, at: &Chosen) -> Result<(), Vec<String>> {
        let mut problems = BTreeSet::new();
        for package in self.each(at)? {
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
                    // A constant that holds itself is found where it is
                    // hashed, with whatever else its value holds.
                    DefinitionKind::Constant(_) => engine.hash(&symbol).map(|_| ()),
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

/// What is wrong, where it is in the text, and the code it is in if
/// that is code a comptime call gave, which the text does not have.
fn report(project: &Project, engine: &Engine, error: &Error) -> String {
    let mut report = project.report(error, engine.blame(error));
    for granting in engine.granting(error) {
        report.push_str(&granting);
    }
    if let Some(given) = engine.shown(error) {
        report.push_str("\nin the code the call gave:\n");
        report.push_str(given.trim_end());
    }
    report
}
