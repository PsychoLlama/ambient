//! `ambient build`: an executable of a program, and how one starts.
//! See "Building" in `docs/spec/packages.md`.
//!
//! An executable is a launcher with a program after it
//! (`ambient_host::launch::Built`), which the launcher looks for in its
//! own file as it starts. The launcher is a host with no compiler
//! ([`LAUNCHER`]), or for a program that compiles code, the command
//! itself.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ambient_host::launch::Built;

use super::launch::{Launch, Linked};
use super::linking::Link;
use super::{Session, entry_of};
use crate::backend::wasm::{self, Wasm};
use crate::engine::Backend;
use crate::host::{self, World};
use crate::pack::Pack;

/// What a program that needs no compiler is put after: the launcher
/// (`ambient-launcher`), built beside the command for the same machine.
pub const LAUNCHER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/launcher"));

/// What building says of a program that both compiles code as it
/// runs and reaches a native bundle: see the TODO under
/// "Distribution" in `docs/spec/externs.md`.
const COMPILES: &str = "a program that packs or compiles code cannot yet reach a native bundle: \
     what loads it is the command, which is linked with none";

/// Runs the program an executable carried in `world`, and gives the
/// code to exit with. What went wrong is written to the world's
/// standard error.
///
/// The command is a launcher that has the compiler, so a program it
/// starts can pack and compile code as well as load it.
#[must_use]
pub fn start(program: &[u8], world: &Rc<RefCell<dyn World>>) -> i32 {
    ambient_host::launch::start_with(program, world, host::compiling)
}

impl Session<'_> {
    /// Makes an executable of `entry` of the package at `path`: at
    /// `output`, or else in the directory the command was run in,
    /// named for the package.
    ///
    /// The program is under `target` of the package, if one is named:
    /// a target that extends the command-line target, and so is built
    /// as that one is. Any other is built by [`Self::build_under`].
    pub(super) fn build(
        &self,
        path: &Path,
        entry: &str,
        target: Option<&str>,
        output: Option<&Path>,
    ) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let Some(target) = Self::target(&project, &engine, target)? else {
            return self.build_under(path, entry, target.unwrap_or_default(), output);
        };
        let linked = launch.link(&entry_of(&project, entry), &target)?;
        let link = self.linking(&project, &linked.parts);
        let link = link.map_err(|problem| vec![problem])?;

        let output = output.map_or_else(|| project.root.clone(), |path| path.display().to_string());
        let program = Self::carried(&launch, &linked);
        // A program that packs or compiles code is started by the
        // command itself, which is the launcher that has the compiler.
        let written = if host::compiles(&linked.parts) {
            if link.is_some() {
                return Err(vec![COMPILES.to_owned()]);
            }
            let Some(command) = &self.launcher else {
                return Err(vec!["the command cannot find its own file".to_owned()]);
            };
            let mut world = self.world.borrow_mut();
            world
                .copy(&command.to_string_lossy(), &output)
                .and_then(|()| world.append(&output, &program))
        } else {
            let executable = self.executable(&launch, &linked, link.as_ref())?;
            self.world.borrow_mut().install(&output, &executable)
        };
        written.map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        self.world.borrow_mut().write(&format!("built {output}\n"));
        Ok(0)
    }

    /// The program of `linked` as an executable carries it.
    fn carried(launch: &Launch, linked: &Linked) -> Vec<u8> {
        let built = Built {
            parts: linked.parts.encode(linked.backend.name()),
            frames: launch.frames(&linked.root),
            trees: linked.trees.as_ref().map(Pack::encode),
        };
        built.carry()
    }

    /// An executable of the program of `linked` that needs no
    /// compiler: the program after the launcher, or after what `link`
    /// says to link of the launcher and native bundles.
    pub(super) fn executable(
        &self,
        launch: &Launch,
        linked: &Linked,
        link: Option<&Link>,
    ) -> Result<Vec<u8>, Vec<String>> {
        let linked_with = link.map(|link| self.linker.link(link)).transpose();
        let launcher = linked_with.map_err(|problem| vec![problem])?;
        let launcher = launcher.as_deref().unwrap_or(LAUNCHER);
        Ok([launcher, &Self::carried(launch, linked)].concat())
    }

    /// Builds `entry` of the package at `path` under `target` of the
    /// same package. See "Builds" in `docs/spec/tools.md`.
    ///
    /// A target built by a tool gives that tool's files, written into
    /// the directory `output`, or else `build` in the package's own.
    /// One with no tool gives the WebAssembly module alone, at
    /// `output` or named for the package: whatever loads it supplies
    /// the native code its target binds, which the module names in
    /// what it exports (`backend::wasm::module`).
    pub(super) fn build_under(
        &self,
        path: &Path,
        entry: &str,
        target: &str,
        output: Option<&Path>,
    ) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let refused = |error| vec![super::report(&project, &engine, &error)];
        let (entry, target) = (entry_of(&project, entry), entry_of(&project, target));
        if let Some(files) = engine.built(&entry, &target).map_err(refused)? {
            let directory = output.map_or_else(|| path.join("build"), Path::to_owned);
            return self.written(&directory, &files);
        }
        let root = engine.program(&entry, &target).map_err(refused)?;
        let parts = engine.link(&root, &Wasm).map_err(refused)?;
        let module = wasm::module(&parts).map_err(|message| vec![message])?;

        let named = || format!("{}.wasm", project.root);
        let output = output.map_or_else(named, |path| path.display().to_string());
        let stored = self.world.borrow_mut().store(&output, &module);
        stored.map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        let built = format!("built {output}, {} bytes\n", module.len());
        self.world.borrow_mut().write(&built);
        Ok(0)
    }
}
