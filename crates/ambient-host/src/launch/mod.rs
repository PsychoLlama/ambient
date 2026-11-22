//! Starting a compiled program: loading it, running it, and the report
//! of one that did not finish. See "Running" in `docs/spec/packages.md`.

mod built;
mod own;
mod said;
mod trace;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

pub use built::{Built, carried, start, start_linked, start_with};
pub use own::{own, own_given};
pub use said::said;
pub use trace::{Frames, Function, describe};

use crate::native::{Natives, Program};
use crate::parts::Parts;
use crate::value::Value;

/// The code a command that could not do its work exits with.
pub const FAILED: i32 = 1;

/// What a program that gave `gave` exits with: the number, if it gave
/// one.
#[must_use]
pub fn code(gave: &Value) -> i32 {
    match gave {
        Value::Int(code) => i32::try_from(*code).unwrap_or(FAILED),
        _ => 0,
    }
}

/// An error with the trace of the throw that brought it to the root
/// after any it had: what catching one gives (`library/errors.ab`).
fn traced(error: &Value, trace: Option<&Value>) -> Value {
    let (Value::Record(fields), Some(Value::List(left))) = (error, trace) else {
        return error.clone();
    };
    let mut fields = fields.clone();
    if let Some(Value::List(frames)) = fields.get_mut("trace") {
        frames.extend(left.iter().cloned());
    }
    Value::Record(fields)
}

/// The extern of `system` that says what an outcome came to.
const SAID: &str = "test.said";

/// The texts of a list, as a set: the hashes `system.test.said` is
/// told are of what ran a test.
fn hashes(given: Option<&Value>) -> BTreeSet<String> {
    let Some(Value::List(given)) = given else {
        return BTreeSet::new();
    };
    let text = |given: &Value| match given {
        Value::String(text) => Some(text.clone()),
        _ => None,
    };
    given.iter().filter_map(text).collect()
}

/// Loads a compiled program and runs it, with `natives` for what its
/// target binds. Gives what it gave, or the fault that ended it and
/// the functions the fault left, which `frames` names if it comes to
/// that.
///
/// # Errors
///
/// Fails with what to report if the program cannot be loaded, faulted,
/// or threw an error nothing dealt with.
pub fn started(
    parts: &Parts,
    natives: &Natives,
    frames: impl Fn() -> Frames,
) -> Result<Value, Vec<String>> {
    let failed = |message: String| vec![message];
    // An error nothing dealt with reaches the root, which keeps it:
    // the program ends there, and is reported as one that faulted.
    let thrown = Rc::new(RefCell::new(None));
    // The command-line target binds `throw` to an extern of its own.
    // What runs a test catches what the test throws (`test.outcome`).
    let kept = Rc::clone(&thrown);
    let natives = natives
        .clone()
        .with(crate::host::PACKAGE, "thrown", move |args| {
            *kept.borrow_mut() = args.first().map(|error| traced(error, args.get(1)));
            Ok(Value::Null)
        });
    // What runs tests says of an outcome as a report of this program
    // names it. The names are worked out only for a program that asks.
    let saying = parts
        .natives
        .iter()
        .any(|native| native.package == crate::host::PACKAGE && native.path == SAID);
    let natives = if saying {
        let named = frames();
        natives.verifiable(crate::host::PACKAGE, SAID, move |args| {
            let none = Value::Null;
            let outcome = args.first().unwrap_or(&none);
            let said = said::said(outcome, &hashes(args.get(1)), &named);
            Ok(Value::List(said.into_iter().map(Value::String).collect()))
        })
    } else {
        natives
    };
    let program = Program::load(parts, &natives).map_err(failed)?;
    let traced = |said: String, error: Option<&Value>| {
        let frames = frames();
        let mut lines = vec![said];
        lines.extend(error.iter().flat_map(|error| frames.trace(error)));
        vec![lines.join("\n")]
    };
    let gave = program.run(&[]).map_err(|fault| {
        let error = program.error();
        let said = match (&error, program.failure()) {
            (_, Some(native)) => format!("{fault:?}: {native}"),
            (Some(error), None) => describe(error),
            (None, None) => format!("{fault:?}"),
        };
        traced(said, error.as_ref())
    })?;
    match thrown.take() {
        Some(error) => Err(traced(describe(&error), Some(&error))),
        None => Ok(gave),
    }
}
