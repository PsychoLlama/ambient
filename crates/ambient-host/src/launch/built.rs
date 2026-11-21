//! An executable of a program, and how one starts. See "Building" in
//! `docs/spec/packages.md`.
//!
//! An executable is a launcher's own file with a program after it:
//! what was compiled, what a report of it names, how long those are,
//! and a mark that says they are there. The launcher looks for the mark
//! in its own file before it reads a command line.

use std::cell::RefCell;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::{FAILED, Frames, code, started};
use crate::host::{self, Linked, World};
use crate::native::{self, Natives};
use crate::pack::Pack;
use crate::parts::Parts;

/// What a file that carries a program ends with.
const MARK: &[u8; 8] = b"\0ambient";

/// How long the end is: the length of the program, then the mark.
const END: u64 = 16;

/// The program an executable carries.
#[derive(Serialize, Deserialize)]
pub struct Built {
    /// The compiled program, as [`Parts::encode`] writes it.
    pub parts: Vec<u8>,
    pub frames: Frames,
    /// The trees of what the program could pack, as [`Pack::encode`]
    /// writes them, if it packs.
    pub trees: Option<Vec<u8>>,
}

impl Built {
    /// What a launcher's file is given after all it holds, to carry
    /// the program: [`carried`] finds it there.
    #[must_use]
    pub fn carry(&self) -> Vec<u8> {
        let mut program = crate::hash::encode(self);
        let size = u64::try_from(program.len()).unwrap_or(u64::MAX);
        program.extend(size.to_le_bytes());
        program.extend(MARK);
        program
    }
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
///
/// The program has the handlers that need no compiler
/// ([`host::natives`]): one that packs or compiles code is started
/// with [`start_with`], by a launcher that has the compiler.
#[must_use]
pub fn start(program: &[u8], world: &Rc<RefCell<dyn World>>) -> i32 {
    start_with(program, world, |natives, _| natives)
}

/// As [`start`], with the handlers `more` adds to those that need no
/// compiler. It is given the trees of what the program could pack, if
/// the program packs.
#[must_use]
pub fn start_with(
    program: &[u8],
    world: &Rc<RefCell<dyn World>>,
    more: impl FnOnce(Natives, Option<Pack>) -> Natives,
) -> i32 {
    start_linked(program, world, &Linked::whole(), more)
}

/// As [`start_with`], in a process that was linked with `linked`.
#[must_use]
pub fn start_linked(
    program: &[u8],
    world: &Rc<RefCell<dyn World>>,
    linked: &Linked,
    more: impl FnOnce(Natives, Option<Pack>) -> Natives,
) -> i32 {
    let run = || {
        let unreadable = |why: String| vec![format!("this is not a program: {why}")];
        let built: Built =
            postcard::from_bytes(program).map_err(|error| unreadable(error.to_string()))?;
        let parts = Parts::decode(&built.parts, native::NAME)
            .map_err(|error| unreadable(error.to_string()))?;
        let trees = built.trees.as_deref().map(Pack::decode).transpose();
        let trees = trees.map_err(|error| unreadable(error.to_string()))?;
        started(
            &parts,
            &more(host::natives_linked(world, linked), trees),
            || built.frames.clone(),
        )
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
