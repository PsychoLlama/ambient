//! Running a program: a package's function, linked and loaded, and the
//! report of one that did not finish. See "Running" in
//! `spec/packages.md`.

use std::cell::RefCell;
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

use super::trace::{Frames, describe};
use crate::backend::native::{Native, Natives, Program};
use crate::engine::{Engine, Parts};
use crate::host;
use crate::pack::Pack;
use crate::project::Project;
use crate::symbol::Symbol;
use crate::value::Value;

/// A package, opened, and the engine that builds it.
pub(super) struct Launch<'a> {
    pub(super) project: &'a Project,
    pub(super) engine: &'a Engine<'a>,
}

/// A program compiled for this machine.
pub(super) struct Linked {
    /// The function the engine wrote to start it.
    pub(super) root: Symbol,
    pub(super) parts: Parts,
    /// The trees of what the program could pack, if it packs.
    pub(super) trees: Option<Pack>,
    pub(super) backend: Native,
}

impl Launch<'_> {
    /// Compiles `entry` as a program under `target`, or says what is
    /// wrong with it.
    pub(super) fn link(&self, entry: &Symbol, target: &Symbol) -> Result<Linked, Vec<String>> {
        let Self { project, engine } = self;
        let refused =
            |error: crate::error::Error| vec![project.report(&error, engine.blame(&error))];
        let root = engine.program(entry, target).map_err(refused)?;
        let backend = Native::new().map_err(|message| vec![message])?;
        let host::Linked { parts, trees } =
            host::link(engine, entry, &root, &backend).map_err(refused)?;
        Ok(Linked {
            root,
            parts,
            trees,
            backend,
        })
    }

    /// The names and places a report of the program of `root` takes.
    pub(super) fn frames(&self, root: &Symbol) -> Frames {
        Frames::reached(self.project, self.engine, root)
    }

    /// Runs `entry` as a program under `target`, with `natives` for
    /// what the target binds. Gives what it gave, or says why it did
    /// not get that far: what is wrong with it, or the fault that
    /// ended it and the functions the fault left.
    pub(super) fn run(
        &self,
        entry: &Symbol,
        target: &Symbol,
        natives: &Natives,
    ) -> Result<Value, Vec<String>> {
        let linked = self.link(entry, target)?;
        started(&linked.parts, natives, || self.frames(&linked.root))
    }
}

/// Loads a compiled program and runs it, with `natives` for what its
/// target binds. Gives what it gave, or the fault that ended it and
/// the functions the fault left, which `frames` names if it comes to
/// that.
pub(super) fn started(
    parts: &Parts,
    natives: &Natives,
    frames: impl Fn() -> Frames,
) -> Result<Value, Vec<String>> {
    let failed = |message: String| vec![message];
    // An error nothing dealt with reaches the root, which keeps it:
    // the program ends there, and is reported as one that faulted.
    let thrown = Rc::new(RefCell::new(None));
    // Each standard target binds `throw` to an extern of its own.
    let externs = [
        (crate::system::PACKAGE, "thrown"),
        (FAULT_PACKAGE, "test.thrown"),
    ];
    let natives = externs
        .into_iter()
        .fold(natives.clone(), |natives, (package, path)| {
            let kept = Rc::clone(&thrown);
            natives.with(package, path, move |args| {
                *kept.borrow_mut() = args.first().cloned();
                Ok(Value::Null)
            })
        });
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
