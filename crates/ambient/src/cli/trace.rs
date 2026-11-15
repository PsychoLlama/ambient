//! What a program that did not finish is reported as: the error that
//! ended it, and where each frame of its trace is in the text. See
//! "Traces" in `spec/errors.md`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

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
/// frames back into names and places. It is all a report needs, so an
/// executable carries one in place of the package it was built from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frames {
    /// The root is a function nobody wrote, so it is held as nothing
    /// and no frame names it.
    functions: BTreeMap<String, Option<Function>>,
}

/// A function as a frame names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Function {
    name: String,
    /// The file it was read from, if it was read from one.
    file: Option<String>,
    /// The line and column of each expression of its body, by the
    /// position a frame gives.
    places: Vec<Option<(usize, usize)>>,
}

impl Function {
    fn written(project: &Project, engine: &Engine, symbol: &Symbol) -> Self {
        let located = project.locate(symbol);
        let place = |node: Option<usize>| {
            let at = located?.1.nodes.get(node?)?;
            Some((at.line, at.column))
        };
        Self {
            name: symbol.to_string(),
            file: located.map(|(file, _)| file.display().to_string()),
            places: engine.origins(symbol).into_iter().map(place).collect(),
        }
    }

    /// The line of a report for a frame at `position` of this one.
    fn line(&self, position: Option<usize>) -> String {
        let place = position.and_then(|position| *self.places.get(position)?);
        match (&self.file, place) {
            (Some(file), Some((line, column))) => {
                format!("  in {} at {file}:{line}:{column}", self.name)
            }
            _ => format!("  in {}", self.name),
        }
    }
}

impl Frames {
    /// Every definition the program of `root` reaches.
    pub(super) fn reached(project: &Project, engine: &Engine, root: &Symbol) -> Self {
        let mut functions = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![root.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            if let Ok(hash) = engine.hash(&symbol) {
                let written =
                    (symbol != *root).then(|| Function::written(project, engine, &symbol));
                functions.insert(hash.to_string(), written);
            }
            if let Ok(open) = engine.check(&symbol) {
                pending.extend(open.slots.iter().cloned());
            }
        }
        Self { functions }
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
        let Some(function) = self.functions.get(hash) else {
            return Some(format!("  in {hash}"));
        };
        let position = match frame.get("position") {
            Some(Value::Int(position)) => usize::try_from(*position).ok(),
            _ => None,
        };
        Some(function.as_ref()?.line(position))
    }
}
