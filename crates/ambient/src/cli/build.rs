//! `ambient build`: an executable of a program, and how one starts.
//! See "Building" in `spec/packages.md`.
//!
//! An executable is the command's own file with a program after it:
//! what was compiled, what a report of it names, how long those are,
//! and a mark that says they are there. The command looks for the mark
//! in its own file before it reads a command line.

use std::cell::RefCell;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::launch::{Launch, started};
use super::trace::Frames;
use super::{FAILED, Session, code, entry_of};
use crate::backend::native::Native;
use crate::engine::{Backend, Engine, Parts};
use crate::host::{self, World};

/// What a file that carries a program ends with.
const MARK: &[u8; 8] = b"\0ambient";

/// How long the end is: the length of the program, then the mark.
const END: u64 = 16;

/// The program an executable carries.
#[derive(Serialize, Deserialize)]
struct Built {
    /// The compiled program, as [`Parts::encode`] writes it.
    parts: Vec<u8>,
    frames: Frames,
}

/// The program a file carries after whatever else it holds, if it
/// carries one.
///
/// # Errors
///
/// The file cannot be read.
pub fn carried(file: &mut (impl Read + Seek)) -> io::Result<Option<Vec<u8>>> {
    let length = file.seek(SeekFrom::End(0))?;
    let Some(end) = length.checked_sub(END) else {
        return Ok(None);
    };
    file.seek(SeekFrom::Start(end))?;
    let (mut size, mut mark) = ([0; 8], [0; 8]);
    file.read_exact(&mut size)?;
    file.read_exact(&mut mark)?;
    let Some(start) = end.checked_sub(u64::from_le_bytes(size)) else {
        return Ok(None);
    };
    if mark != *MARK {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(start))?;
    let mut program = Vec::new();
    file.take(end - start).read_to_end(&mut program)?;
    Ok(Some(program))
}

/// Runs the program an executable carried in `world`, and gives the
/// code to exit with. What went wrong is written to the world's
/// standard error.
#[must_use]
pub fn start(program: &[u8], world: &Rc<RefCell<dyn World>>) -> i32 {
    let run = || {
        let unreadable = |why: String| vec![format!("this is not a program: {why}")];
        let built: Built =
            postcard::from_bytes(program).map_err(|error| unreadable(error.to_string()))?;
        let backend = Native::new().map_err(unreadable)?;
        let parts = Parts::decode(&built.parts, backend.name())
            .map_err(|error| unreadable(error.to_string()))?;
        started(&parts, &host::natives(world), || built.frames.clone())
    };
    run().map_or_else(
        |problems: Vec<String>| {
            let mut world = world.borrow_mut();
            for problem in problems {
                world.warn(&format!("error: {problem}\n"));
            }
            FAILED
        },
        |gave| code(&gave),
    )
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
            parts: linked.parts.encode(&linked.backend),
            frames: launch.frames(&linked.root),
        };
        let mut program = crate::hash::encode(&built);
        let size = u64::try_from(program.len()).unwrap_or(u64::MAX);
        program.extend(size.to_le_bytes());
        program.extend(MARK);

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
