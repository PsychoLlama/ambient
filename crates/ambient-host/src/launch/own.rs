//! A launcher's start: the program its own file carries, run in the
//! world of this process.

use std::cell::RefCell;
use std::ffi::OsString;
use std::process::ExitCode;
use std::rc::Rc;

use super::{FAILED, carried};
use crate::host::{Linked, System, World};

/// The program the file of this process carries, if it carries one.
fn program() -> Option<Vec<u8>> {
    let own = std::env::current_exe().ok()?;
    let mut file = std::fs::File::open(own).ok()?;
    carried(&mut file).ok()?
}

/// Runs the program the file of this process carries with `start`, and
/// gives what to exit with; or nothing, if the file carries none.
///
/// The program is started with everything the process was given: no
/// argument is the launcher's.
pub fn own(start: impl FnOnce(&[u8], &Rc<RefCell<dyn World>>) -> i32) -> Option<ExitCode> {
    let code = own_given(std::env::args_os().skip(1), &Linked::whole(), start)?;
    Some(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// [`own`], for a process that is told its arguments, each after the
/// name it was started by: one whose `main` is not this language's is
/// not everywhere told them any other way. Its world is that of a
/// process linked with `linked`.
pub fn own_given(
    args: impl Iterator<Item = OsString>,
    linked: &Linked,
    start: impl FnOnce(&[u8], &Rc<RefCell<dyn World>>) -> i32,
) -> Option<i32> {
    let program = program()?;
    // An argument that is not text cannot be handed to the program.
    let Some(args) = args.map(|arg| arg.into_string().ok()).collect() else {
        System::linked(Vec::new(), linked).warn("error: an argument is not text\n");
        return Some(FAILED);
    };
    let world: Rc<RefCell<dyn World>> = Rc::new(RefCell::new(System::linked(args, linked)));
    Some(start(&program, &world))
}
