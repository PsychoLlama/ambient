//! `ambient build`: an executable of a program, and how one starts.
//! See "Building" in `docs/spec/packages.md`.
//!
//! An executable is a launcher with a program after it
//! (`ambient_host::launch::Built`), which the launcher looks for in its
//! own file as it starts. The launcher is a host with no compiler,
//! linked for the program (`cli/linking.rs`), or for a program that
//! compiles code, the command itself.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ambient_host::launch::Built;

use super::launch::{Launch, Linked};
use super::linking::Link;
use super::picked::{self, Picked};
use super::{Chosen, Session};
use crate::backend::wasm::{self, Wasm};
use crate::engine::Backend;
use crate::host::{self, World};
use crate::pack::Pack;
use crate::project::Binary;

/// What building says of a program that both compiles code as it
/// runs and reaches a native bundle: see the TODO under
/// "Distribution" in `docs/spec/externs.md`.
const COMPILES: &str = "a program that packs or compiles code cannot yet reach a native bundle: \
     what loads it is the command, which is linked with none";

/// What building says where it is told one place for several binaries.
const SEVERAL: &str = "`--output` is where one binary is put: say which with `--bin`";

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
    /// Builds the binary `bin` of the packages `chosen` is, or else
    /// every binary they have: at `output`, which is where one is put,
    /// or else where each is put when the command says nothing.
    pub(super) fn build(
        &self,
        chosen: &Chosen,
        bin: Option<&str>,
        output: Option<&Path>,
    ) -> Result<i32, Vec<String>> {
        let offered = self.offered(chosen)?;
        let picked = picked::every(&offered, bin)?;
        if output.is_some() && picked.len() > 1 {
            return Err(vec![SEVERAL.to_owned()]);
        }
        for picked in picked {
            self.built(picked, output)?;
        }
        Ok(0)
    }

    /// Makes an executable of a binary: at `output`, or else in the
    /// directory the command was run in, named as the binary says.
    ///
    /// A binary whose target does not extend the command-line target
    /// is built by [`Self::build_under`].
    fn built(&self, picked: Picked, output: Option<&Path>) -> Result<(), Vec<String>> {
        let Picked { package, binary } = picked;
        let project = &package.project;
        let engine = self.engine(project);
        let launch = Launch {
            project,
            engine: &engine,
        };
        let Some(target) = Self::target(project, &engine, binary)? else {
            return self.build_under(&launch, &package.directory, binary, output);
        };
        let linked = launch.link(&binary.entry, &target)?;
        let link = self.linking(project, &linked.parts);
        let link = link.map_err(|problem| vec![problem])?;

        let output = output.map_or_else(|| binary.name.clone(), |path| path.display().to_string());
        let program = Self::carried(&launch, &linked);
        self.room(&output)?;
        // A program that packs or compiles code is started by the
        // command itself, which is the launcher that has the compiler.
        let written = if host::compiles(&linked.parts) {
            if !link.bundles.is_empty() {
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
            let executable = self.executable(&launch, &linked, &link)?;
            self.world.borrow_mut().install(&output, &executable)
        };
        written.map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        self.world.borrow_mut().write(&format!("built {output}\n"));
        Ok(())
    }

    /// Makes the directory the file at `output` is to be written in,
    /// if it is not there: a build is often the first thing put in one.
    fn room(&self, output: &str) -> Result<(), Vec<String>> {
        let parent = Path::new(output).parent();
        let Some(parent) = parent.filter(|parent| !parent.as_os_str().is_empty()) else {
            return Ok(());
        };
        let made = self
            .world
            .borrow_mut()
            .make_directory(&parent.to_string_lossy());
        made.map_err(|failure| vec![format!("{}: {}", parent.display(), failure.message)])
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
    /// compiler: the program after what `link` says to link of the
    /// launcher and native bundles.
    pub(super) fn executable(
        &self,
        launch: &Launch,
        linked: &Linked,
        link: &Link,
    ) -> Result<Vec<u8>, Vec<String>> {
        let launcher = self.linker.link(link).map_err(|problem| vec![problem])?;
        Ok([launcher, Self::carried(launch, linked)].concat())
    }

    /// Builds `binary` of the package at `path` under the target it
    /// names. See "Builds" in `docs/spec/tools.md`.
    ///
    /// A target built by a tool gives that tool's files, written into
    /// the directory `output`, or else `build` in the package's own.
    /// One with no tool gives the WebAssembly module alone, at
    /// `output` or named as the binary says: whatever loads it supplies
    /// the native code its target binds, which the module names in
    /// what it exports (`backend::wasm::module`).
    fn build_under(
        &self,
        launch: &Launch,
        path: &Path,
        binary: &Binary,
        output: Option<&Path>,
    ) -> Result<(), Vec<String>> {
        let Launch { project, engine } = launch;
        let refused = |error| vec![super::report(project, engine, &error)];
        // Only a target that was named is one the command line's is not.
        let target = binary.target.clone().unwrap_or_else(crate::system::target);
        if let Some(files) = engine.built(&binary.entry, &target).map_err(refused)? {
            let directory = output.map_or_else(|| path.join("build"), Path::to_owned);
            return self.written(&directory, &files).map(|_| ());
        }
        let root = engine.program(&binary.entry, &target).map_err(refused)?;
        let parts = engine.link(&root, &Wasm).map_err(refused)?;
        let module = wasm::module(&parts).map_err(|message| vec![message])?;

        let named = || format!("{}.wasm", binary.name);
        let output = output.map_or_else(named, |path| path.display().to_string());
        self.room(&output)?;
        let stored = self.world.borrow_mut().store(&output, &module);
        stored.map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        let built = format!("built {output}, {} bytes\n", module.len());
        self.world.borrow_mut().write(&built);
        Ok(())
    }
}
