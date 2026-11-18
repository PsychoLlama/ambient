//! `ambient build`: an executable of a program, and how one starts.
//! See "Building" in `spec/packages.md`.
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

use super::launch::Launch;
use super::{Session, entry_of};
use crate::backend::wasm::{self, Wasm};
use crate::engine::Backend;
use crate::host::{self, World};
use crate::pack::Pack;

/// What a program that needs no compiler is put after: the launcher
/// (`ambient-launcher`), built beside the command for the same machine.
pub const LAUNCHER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/launcher"));

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
    pub(super) fn build(
        &self,
        path: &Path,
        entry: &str,
        output: Option<&Path>,
    ) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let linked = launch.link(&entry_of(&project, entry), &crate::system::target())?;
        let built = Built {
            parts: linked.parts.encode(linked.backend.name()),
            frames: launch.frames(&linked.root),
            trees: linked.trees.as_ref().map(Pack::encode),
        };
        let program = built.carry();

        let output = output.map_or_else(|| project.root.clone(), |path| path.display().to_string());
        let mut world = self.world.borrow_mut();
        // A program that packs or compiles code is started by the
        // command itself, which is the launcher that has the compiler.
        let written = if host::compiles(&linked.parts) {
            let Some(command) = &self.launcher else {
                return Err(vec!["the command cannot find its own file".to_owned()]);
            };
            world
                .copy(&command.to_string_lossy(), &output)
                .and_then(|()| world.append(&output, &program))
        } else {
            world.install(&output, &[LAUNCHER, &program].concat())
        };
        written.map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        world.write(&format!("built {output}\n"));
        Ok(0)
    }

    /// Builds `entry` of the package at `path` under `target` of the
    /// same package. See "Builds" in `spec/tools.md`.
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
