//! A launcher's start: the program its own file carries, run in the
//! world of this process.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use super::carried;
use crate::host::{System, World};

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
    let program = program()?;
    // An argument that is not text cannot be handed to the program.
    let text = |arg: std::ffi::OsString| arg.into_string().ok();
    let Some(args) = std::env::args_os().skip(1).map(text).collect() else {
        System::new(Vec::new()).warn("error: an argument is not text\n");
        return Some(ExitCode::FAILURE);
    };
    let world: Rc<RefCell<dyn World>> = Rc::new(RefCell::new(System::new(args)));
    let code = start(&program, &world);
    Some(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}
