//! What a report of a program names: where each frame of a trace is in
//! the text. See "Traces" in `docs/spec/errors.md`.
//!
//! The names and places are plain data (`ambient_host::launch::Frames`),
//! which a host with no compiler reads a trace by; gathering them takes
//! the engine and the package.

use std::collections::BTreeSet;

use ambient_host::launch::{Frames, Function};

use crate::engine::Engine;
use crate::project::Project;
use crate::surface::GENERATOR;
use crate::symbol::Symbol;

/// A function as a frame names it: its name, and where it was written.
/// One a comptime call gave was written nowhere: every place in it is
/// where the call is.
fn written(project: &Project, engine: &Engine, symbol: &Symbol) -> Function {
    let call = symbol.module.value(GENERATOR);
    let located = project.locate(symbol).or_else(|| project.locate(&call));
    let given = engine.is_given(symbol);
    let place = |node: Option<usize>| {
        let placed = located?.1;
        let at = if given {
            &placed.at
        } else {
            placed.nodes.get(node?)?
        };
        Some((at.line, at.column))
    };
    Function {
        name: symbol.to_string(),
        file: located.map(|(file, _)| file.display().to_string()),
        places: engine.origins(symbol).into_iter().map(place).collect(),
    }
}

/// Every definition the program of `root` reaches.
pub(super) fn reached(project: &Project, engine: &Engine, root: &Symbol) -> Frames {
    let mut frames = Frames::default();
    walk(engine, root, |symbol, hash| {
        // The root is a function nobody wrote.
        let written = (symbol != root).then(|| written(project, engine, symbol));
        frames.name(hash, written);
    });
    frames
}

/// The hash of each definition `symbol` reaches, itself included.
pub(super) fn reach(engine: &Engine, symbol: &Symbol) -> BTreeSet<String> {
    let mut hashes = BTreeSet::new();
    walk(engine, symbol, |_, hash| {
        hashes.insert(hash);
    });
    hashes
}

/// Calls `each` with every definition `from` reaches, and its hash.
fn walk(engine: &Engine, from: &Symbol, mut each: impl FnMut(&Symbol, String)) {
    let mut seen = BTreeSet::new();
    let mut pending = vec![from.clone()];
    while let Some(symbol) = pending.pop() {
        if !seen.insert(symbol.clone()) {
            continue;
        }
        if let Ok(hash) = engine.hash(&symbol) {
            each(&symbol, hash.to_string());
        }
        if let Ok(open) = engine.check(&symbol) {
            pending.extend(open.slots.iter().cloned());
        }
    }
}
