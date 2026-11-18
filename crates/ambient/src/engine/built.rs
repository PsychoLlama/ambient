//! Builds: the files a program is built as under a target, which the
//! target's tool and the build steps of the packages it reaches give.
//! See "Builds" in `spec/tools.md`.

use std::collections::{BTreeSet, HashSet};

use ambient_runtime::FAULT_PACKAGE;

use super::Engine;
use super::asked::{MODULE, record};
use crate::checker;
use crate::error::Error;
use crate::surface::{DefinitionKind, Reference};
use crate::symbol::{ModuleId, Symbol};
use crate::types::{Ty, abilities};
use crate::value::Value;

/// The files of a build: each a path and what the file holds.
pub type Files = Vec<(String, Vec<u8>)>;

/// The files a tool gave. What a checked tool gives is files,
/// whatever it did.
fn files_of(given: &Value) -> Files {
    let file = |file: &Value| {
        let Value::Record(fields) = file else {
            return None;
        };
        match (fields.get("path"), fields.get("bytes")) {
            (Some(Value::String(path)), Some(Value::Bytes(bytes))) => {
                Some((path.clone(), bytes.clone()))
            }
            _ => None,
        }
    };
    match given {
        Value::List(given) => given.iter().filter_map(file).collect(),
        _ => Files::new(),
    }
}

/// Files as a `build.Files`.
fn handed(files: &Files) -> Value {
    let file = |(path, bytes): &(String, Vec<u8>)| {
        record([
            ("path", Value::String(path.clone())),
            ("bytes", Value::Bytes(bytes.clone())),
        ])
    };
    Value::List(files.iter().map(file).collect())
}

impl Engine<'_> {
    /// The files `entry` is built as under `target`, each a path and
    /// what it holds, if the target is built by a tool: what each build
    /// step for the target gives, and then what the tool gives, which
    /// is handed the steps' files.
    ///
    /// # Errors
    ///
    /// Fails if what the target is built by is not a tool, a step is
    /// not one, two files are given for one path, or as [`Self::tool`]
    /// does.
    pub fn built(&self, entry: &Symbol, target: &Symbol) -> Result<Option<Files>, Error> {
        let Some(tool) = self.builder(target)? else {
            return Ok(None);
        };
        if !self.takes(&tool, true)? {
            return Err(Error::NotTool(tool));
        }
        let args = [checker::reference(entry), checker::reference(target)];
        let mut files = Files::new();
        for step in self.steps(entry, target)? {
            if !self.takes(&step, false)? {
                return Err(Error::NotStep(step));
            }
            files.extend(files_of(&self.tool(&step, &args)?));
        }
        let [entered, under] = args;
        let given = self.tool(&tool, &[entered, under, handed(&files)])?;
        files.extend(files_of(&given));

        let mut paths = BTreeSet::new();
        match files.iter().find(|(path, _)| !paths.insert(path)) {
            Some((path, _)) => Err(Error::BuiltTwice {
                symbol: target.clone(),
                path: path.clone(),
            }),
            None => Ok(Some(files)),
        }
    }

    /// Whether `tool` is typed as what a build runs: it takes a
    /// reference to an entry and one to a target, and the files the
    /// steps gave if it is a target's (`last`), and gives files.
    fn takes(&self, tool: &Symbol, last: bool) -> Result<bool, Error> {
        let library = |module: &str, name: &str| {
            let named = ModuleId::new(FAULT_PACKAGE, module).ty(name);
            Ty::Named(Reference::Definition(named), Vec::new())
        };
        let (reference, files) = (library("tree", "Reference"), library(MODULE, "Files"));
        let mut params = vec![reference.clone(), reference];
        params.extend(last.then(|| files.clone()));
        Ok(self.signature(tool)?.is_some_and(|(signature, _)| {
            signature.type_params.is_empty() && signature.params == params && signature.ret == files
        }))
    }

    /// The build steps that run in a build of `entry` under `target`:
    /// each declared for the target or one it extends, by a package
    /// the program reaches.
    fn steps(&self, entry: &Symbol, target: &Symbol) -> Result<Vec<Symbol>, Error> {
        let mut extended = Vec::new();
        self.extended(target, &mut extended)?;
        let mut steps = self.sources.steps();
        steps.retain(|(target, _)| extended.contains(target));
        if steps.is_empty() {
            return Ok(Vec::new());
        }
        let reached = self.packages(&self.program(entry, target)?)?;
        let tools = steps.into_iter().map(|(_, tool)| tool);
        let declares = |tool: &Symbol| self.sources.identity(&tool.module.package);
        Ok(tools
            .filter(|tool| reached.contains(&declares(tool)))
            .collect())
    }

    /// Adds `target` and every target it extends to `found`.
    fn extended(&self, target: &Symbol, found: &mut Vec<Symbol>) -> Result<(), Error> {
        if found.contains(target) {
            return Ok(());
        }
        let resolved = self.resolve(target)?;
        let DefinitionKind::Target(definition) = &resolved.definition.kind else {
            return Err(Error::NotTarget(target.clone()));
        };
        found.push(target.clone());
        for extended in abilities(&definition.extends) {
            self.extended(&extended, found)?;
        }
        Ok(())
    }

    /// The IDs of the packages `root` reaches: those of the definitions
    /// it reaches, and of the keys they are annotated with, which is
    /// how a program reaches a package it only says something to.
    fn packages(&self, root: &Symbol) -> Result<HashSet<String>, Error> {
        let mut reached = HashSet::new();
        let mut seen = HashSet::new();
        let mut pending = vec![root.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            let open = self.check(&symbol)?;
            reached.insert(self.sources.identity(&symbol.module.package));
            reached.extend(open.annotations.iter().map(|said| said.package.clone()));
            pending.extend(open.slots.iter().cloned());
        }
        Ok(reached)
    }

    /// Runs `tool` with `args` under the build target, and gives what
    /// it gives. See "Tools" in `spec/tools.md`.
    ///
    /// # Errors
    ///
    /// Fails if the tool takes an ability its package is not granted,
    /// does not compile for this machine, or faults.
    pub fn tool(&self, tool: &Symbol, args: &[Value]) -> Result<Value, Error> {
        let package = &tool.module.package;
        let granted = self.sources.grants(package);
        let row = self.signature(tool)?.map(|(signature, _)| signature.row);
        if let Some(ability) = row
            .iter()
            .flatten()
            .find(|ability| !granted.contains(ability))
        {
            return Err(Error::ToolUngranted {
                symbol: tool.clone(),
                ability: ability.clone(),
            });
        }
        self.run(package, tool, args)
    }
}
