//! Linking a program: a package's function, compiled for this machine, and the
//! report of one that did not finish. See "Running" in
//! `docs/spec/packages.md`.

use ambient_host::launch::Frames;

use super::trace;
use crate::backend::native::Native;
use crate::engine::{Engine, Parts};
use crate::host;
use crate::pack::Pack;
use crate::project::Project;
use crate::symbol::Symbol;

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
        let refused = |error: crate::error::Error| vec![super::report(project, engine, &error)];
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
        trace::reached(self.project, self.engine, root)
    }
}
