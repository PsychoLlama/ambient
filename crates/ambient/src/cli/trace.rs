//! What a program that did not finish is reported as: the error that
//! ended it, and where each frame of its trace is in the text. See
//! "Traces" in `spec/errors.md`.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::Engine;
use crate::project::Project;
use crate::symbol::Symbol;
use crate::value::Value;

/// What an error value says went wrong: its kind and its message.
pub(super) fn describe(error: &Value) -> String {
    let Value::Record(fields) = error else {
        return "the program faulted".to_owned();
    };
    let kind = match fields.get("kind") {
        Some(Value::Tag { tag, .. }) => tag.as_str(),
        _ => "fault",
    };
    match fields.get("message") {
        Some(Value::String(message)) if !message.is_empty() => format!("{kind}: {message}"),
        _ => kind.to_owned(),
    }
}

/// The definitions a program reaches, by hash: what turns a trace's
/// frames back into names and places.
pub(super) struct Named<'a> {
    project: &'a Project,
    engine: &'a Engine<'a>,
    /// The root is a function nobody wrote, so it has no name here and
    /// no frame names it.
    names: BTreeMap<String, Option<Symbol>>,
}

impl<'a> Named<'a> {
    pub(super) fn reached(project: &'a Project, engine: &'a Engine<'a>, root: &Symbol) -> Self {
        let mut names = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![root.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            if let Ok(hash) = engine.hash(&symbol) {
                let written = (symbol != *root).then(|| symbol.clone());
                names.insert(hash.to_string(), written);
            }
            if let Ok(open) = engine.check(&symbol) {
                pending.extend(open.slots.iter().cloned());
            }
        }
        Self {
            project,
            engine,
            names,
        }
    }

    /// A line for each frame of an error's trace, nearest first: the
    /// function it is in, and where in the text if that was read.
    pub(super) fn trace(&self, error: &Value) -> Vec<String> {
        let Value::Record(fields) = error else {
            return Vec::new();
        };
        let Some(Value::List(frames)) = fields.get("trace") else {
            return Vec::new();
        };
        frames
            .iter()
            .filter_map(|frame| self.frame(frame))
            .collect()
    }

    fn frame(&self, frame: &Value) -> Option<String> {
        let Value::Record(frame) = frame else {
            return None;
        };
        let Some(Value::String(hash)) = frame.get("definition") else {
            return None;
        };
        let Some(symbol) = self.names.get(hash) else {
            return Some(format!("  in {hash}"));
        };
        let symbol = symbol.as_ref()?;
        let position = match frame.get("position") {
            Some(Value::Int(position)) => usize::try_from(*position).ok(),
            _ => None,
        };
        let node = position.and_then(|position| self.engine.origin(symbol, position));
        Some(
            match node.and_then(|node| self.project.place(symbol, node)) {
                Some(place) => format!("  in {symbol} at {place}"),
                None => format!("  in {symbol}"),
            },
        )
    }
}
