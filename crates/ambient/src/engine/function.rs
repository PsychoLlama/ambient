//! Checking a function: one that was written, a program's root, or the
//! call a type is derived from. See `docs/spec/definitions.md` and
//! `docs/spec/comptime.md`.
//!
//! A function's check may read what its key does not cover: what a
//! comptime call observed, and the definitions that code one gave
//! names. Both are kept with the result and looked at again before it
//! is reused.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::run::Dependency;
use super::{Engine, Query, Stage};
use crate::checker;
use crate::core::Open;
use crate::error::Error;
use crate::hash::{Hash, Hasher, encode};
use crate::surface::{DefinitionKind, Function, Reference};
use crate::symbol::{Namespace, Symbol};

/// A checked function, kept with the runs its content came from.
#[derive(Serialize, Deserialize)]
struct Checked {
    result: Result<Open, Error>,
    runs: Vec<Dependency>,
    /// Where things are in the body: of the key's making, as the tree is.
    noted: checker::Noted,
    /// What the check read of each definition that spliced code named,
    /// which no key covers: the key was made before the code was.
    reached: Vec<(Symbol, Hash)>,
}

impl Engine<'_> {
    /// What checking reads of a definition it names: the hash of a
    /// type, an ability, or a key, and the interface of anything else.
    fn seen_of(&self, named: &Symbol) -> Result<Hash, Error> {
        match named.namespace {
            Namespace::Value => Ok(self.interface(named)?.digest),
            _ => self.hash(named),
        }
    }

    /// Checks a function: one that was written, or a program's root.
    pub(super) fn check_function(
        &self,
        symbol: &Symbol,
        function: &Function<Reference>,
        used: Vec<Symbol>,
        mut key: Hasher,
        root: bool,
    ) -> Query<Open> {
        let mut sends = Vec::new();
        function.body.sends(&mut |name| {
            if !sends.iter().any(|sent| sent == name) {
                sends.push(name.to_owned());
            }
        });
        // A using reads a key its tree does not name.
        let mut used = used;
        if function.body.releases() {
            used.push(crate::library::dispose());
        }
        let env = self.environment(used.clone(), &sends, &mut key)?;
        // What a comptime call returns is content of this definition, and
        // depends on everything its callee is.
        let mut callees = Vec::new();
        function.body.comptime_callees(&mut |name| {
            if let Reference::Definition(callee) = name {
                callees.push(callee.clone());
            }
        });
        for callee in &callees {
            key.hash(&self.hash(callee)?);
        }
        // What a comptime call may take is the package's grants.
        if !callees.is_empty() {
            key.bytes(&encode(&self.sources.grants(&symbol.module.package)));
        }
        // A comptime call that takes abilities observed the world. Its
        // result is kept with the definition and looked at again before
        // the definition is reused.
        let still = |stored: &Result<Checked, Error>| {
            stored.as_ref().is_ok_and(|checked| {
                // What spliced code named must be what it was, and
                // still be something the package may name.
                let read = |(reached, seen): &(Symbol, Hash)| {
                    self.seen_of(reached).ok() == Some(*seen)
                        && self.nameable(&symbol.module.package, reached)
                };
                self.still_gives(&checked.runs) && checked.reached.iter().all(read)
            })
        };
        let checked = self.stage_if(Stage::Check, symbol, key.finish(), still, || {
            self.begin_runs();
            let mut noted = checker::Noted::default();
            let mut env = Some(env);
            let mut reached: Vec<Symbol> = Vec::new();
            // Code a comptime call gave may name what the body does
            // not: the check is begun again knowing of it, until it
            // names nothing new.
            let result = loop {
                let Some(known) = env.take() else {
                    break Err(Error::UnknownSymbol(symbol.clone()));
                };
                match checker::check_function(symbol, function, known, self, root, &mut noted) {
                    Err(Error::Reaches { reached: more, .. })
                        if more.iter().any(|named| !reached.contains(named)) =>
                    {
                        reached.extend(more);
                        reached.sort();
                        reached.dedup();
                        // The check begins again, and makes its runs
                        // again: those of this turn are not kept twice.
                        self.end_runs();
                        self.begin_runs();
                        let all = used.iter().chain(&reached).cloned().collect();
                        match self.environment(all, &sends, &mut Hasher::new("reached")) {
                            Ok(wider) => env = Some(wider),
                            Err(error) => break Err(error),
                        }
                    }
                    result => break result,
                }
            };
            let runs = self.end_runs();
            reached.extend(noted.reached.iter().cloned());
            reached.sort();
            reached.dedup();
            let read = |named: Symbol| {
                Ok((self.seen_of(&named)?, named)).map(|(seen, named)| (named, seen))
            };
            Ok(Checked {
                result,
                runs,
                noted,
                reached: reached
                    .into_iter()
                    .map(read)
                    .collect::<Result<_, Error>>()?,
            })
        })?;
        self.note(symbol, &checked.noted);
        checked.result.clone().map(Arc::new)
    }

    /// Keeps what checking a function noted beside its tree: where its
    /// first error is, the code a comptime call gave that it is in,
    /// and what each expression was made from.
    fn note(&self, symbol: &Symbol, noted: &checker::Noted) {
        // A node of what was checked is numbered as the resolved body
        // has it, and is kept as the text does.
        let resolved = self.resolve(&symbol.lambda(&[])).ok();
        let written = matches!(
            resolved
                .as_deref()
                .map(|resolved| &resolved.definition.kind),
            Some(DefinitionKind::Function(_))
        );
        let resolved = resolved.filter(|_| written);
        let place = |node: usize| resolved.as_ref().map_or(node, |found| found.written(node));
        if let Some(at) = noted.at {
            self.blamed.borrow_mut().insert(symbol.clone(), place(at));
        }
        if let Some(given) = &noted.given {
            let given = given.clone();
            self.spliced.borrow_mut().insert(symbol.clone(), given);
        }
        let mut origins = self.origins.borrow_mut();
        if !origins.contains_key(symbol) {
            let mut made = noted.origins.clone();
            for node in made.values_mut().flatten() {
                let at = place(usize::try_from(*node).unwrap_or(usize::MAX));
                *node = u32::try_from(at).unwrap_or(u32::MAX);
            }
            origins.insert(symbol.clone(), made);
        }
    }
}
