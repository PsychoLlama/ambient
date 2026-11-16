//! `ambient build`: an executable of a program, and how one starts.
//! See "Building" in `spec/packages.md`.
//!
//! An executable is the command's own file with a program after it
//! (`ambient_host::launch::Built`), which the command looks for in its
//! own file before it reads a command line.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ambient_host::launch::Built;

use super::launch::Launch;
use super::{Session, entry_of};
use crate::engine::{Backend, Engine};
use crate::host::{self, World};
use crate::pack::Pack;

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
        let Some(launcher) = &self.launcher else {
            return Err(vec!["the command cannot find its own file".to_owned()]);
        };
        let project = self.open(path)?;
        let engine = Engine::new(&project.workspace, self.store);
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
        world
            .copy(&launcher.to_string_lossy(), &output)
            .and_then(|()| world.append(&output, &program))
            .map_err(|failure| vec![format!("{output}: {}", failure.message)])?;
        world.write(&format!("built {output}\n"));
        Ok(0)
    }
}
