//! Programs: an entry and a target. See `docs/spec/targets.md`.
//!
//! A program's root is a function nobody wrote: the entry, called under
//! the target's bindings. It is checked, hashed, and compiled like any
//! other definition.

use std::collections::{BTreeMap, BTreeSet};

use super::{Engine, Query};
use crate::core::Open;
use crate::error::Error;
use crate::hash::{Hasher, digest};
use crate::surface::{DefinitionKind, Expr, Function, Key, Param, Reference, Type};
use crate::symbol::{Namespace, Symbol};
use crate::types::{abilities, layers};

/// What a root was made from.
#[derive(Clone)]
pub(super) struct Root {
    entry: Symbol,
    target: Symbol,
    /// A function the entry is handed to, which the root calls in its
    /// place: what runs a test, and gives what it came to.
    within: Option<Symbol>,
}

impl Engine<'_> {
    /// The root of the program that runs `entry` under `target`: a symbol
    /// that can be checked, linked, and run like a function's.
    ///
    /// # Errors
    ///
    /// Fails if `entry` is not a function without type parameters.
    pub fn program(&self, entry: &Symbol, target: &Symbol) -> Result<Symbol, Error> {
        Ok(self.rooted(Root {
            entry: entry.clone(),
            target: target.clone(),
            within: None,
        }))
    }

    /// The root of the program that hands `entry` to `within` under
    /// `target`, and gives what `within` gives. What `within` expects
    /// of the function it is handed it provides itself, so the target
    /// need bind only the rest of the entry's row, and `within`'s own.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::program`] does.
    pub fn program_within(
        &self,
        entry: &Symbol,
        within: &Symbol,
        target: &Symbol,
    ) -> Result<Symbol, Error> {
        Ok(self.rooted(Root {
            entry: entry.clone(),
            target: target.clone(),
            within: Some(within.clone()),
        }))
    }

    /// A symbol for `root`, by which it is checked, linked, and run.
    fn rooted(&self, root: Root) -> Symbol {
        let Root {
            entry,
            target,
            within,
        } = &root;
        // No builder accepts this name, so it cannot be a definition's.
        let name = match within {
            None => format!("{}@{entry}", target.name),
            Some(within) => format!("{}@{within}@{entry}", target.name),
        };
        let symbol = target.module.symbol(Namespace::Value, &name);
        self.roots.borrow_mut().insert(symbol.clone(), root);
        symbol
    }

    pub(super) fn root_of(&self, symbol: &Symbol) -> Option<Root> {
        self.roots.borrow().get(symbol).cloned()
    }

    pub(super) fn check_root(&self, symbol: &Symbol, root: &Root) -> Query<Open> {
        let function = self.root(root)?;
        let mut key = Hasher::new("check");
        key.hash(&digest("root", &function));
        let mut used: Vec<Symbol> = Vec::new();
        let mut visit = |name: &Reference| {
            if let Reference::Definition(symbol) = name
                && !used.contains(symbol)
            {
                used.push(symbol.clone());
            }
        };
        function.header_names(&mut visit);
        function.body.names(&mut visit);
        self.check_function(symbol, &function, used, key, true)
    }

    /// Every binding of a target, those of the targets it extends
    /// included, by ability.
    ///
    /// # Errors
    ///
    /// Fails if `target` is not a target, extends itself, or extends
    /// targets that disagree about an ability it does not bind itself.
    pub fn bindings(&self, target: &Symbol) -> Result<Vec<(Symbol, Symbol)>, Error> {
        self.flatten(target, &mut Vec::new())
    }

    /// The tool that builds a program under `target`: the one it
    /// names, or else the one the targets it extends are built by.
    /// See "Builds" in `docs/spec/tools.md`.
    ///
    /// # Errors
    ///
    /// Fails if `target` is not a target, extends itself, or extends
    /// targets built by different tools without naming one.
    pub fn builder(&self, target: &Symbol) -> Result<Option<Symbol>, Error> {
        self.built_by(target, &mut Vec::new())
    }

    /// Whether `target` is `base`, or extends it through however many
    /// targets: whether a program under it is one under `base` with
    /// bindings added and replaced.
    ///
    /// # Errors
    ///
    /// Fails if `target` is not a target, or extends itself.
    pub fn extends(&self, target: &Symbol, base: &Symbol) -> Result<bool, Error> {
        self.extending(target, base, &mut Vec::new())
    }

    fn extending(
        &self,
        target: &Symbol,
        base: &Symbol,
        visiting: &mut Vec<Symbol>,
    ) -> Result<bool, Error> {
        if target == base {
            return Ok(true);
        }
        if visiting.contains(target) {
            return Err(Error::TargetCycle(target.clone()));
        }
        let resolved = self.resolve(target)?;
        let DefinitionKind::Target(definition) = &resolved.definition.kind else {
            return Err(Error::NotTarget(target.clone()));
        };
        visiting.push(target.clone());
        let mut found = false;
        for extended in abilities(&definition.extends) {
            found = found || self.extending(&extended, base, visiting)?;
        }
        visiting.pop();
        Ok(found)
    }

    fn built_by(
        &self,
        target: &Symbol,
        visiting: &mut Vec<Symbol>,
    ) -> Result<Option<Symbol>, Error> {
        if visiting.contains(target) {
            return Err(Error::TargetCycle(target.clone()));
        }
        let resolved = self.resolve(target)?;
        let DefinitionKind::Target(definition) = &resolved.definition.kind else {
            return Err(Error::NotTarget(target.clone()));
        };
        if let Some(Reference::Definition(tool)) = &definition.tool {
            return Ok(Some(tool.clone()));
        }
        let mut tools = BTreeSet::new();
        visiting.push(target.clone());
        for extended in abilities(&definition.extends) {
            tools.extend(self.built_by(&extended, visiting)?);
        }
        visiting.pop();
        if tools.len() > 1 {
            return Err(Error::ToolConflict(target.clone()));
        }
        Ok(tools.into_iter().next())
    }

    fn flatten(
        &self,
        target: &Symbol,
        visiting: &mut Vec<Symbol>,
    ) -> Result<Vec<(Symbol, Symbol)>, Error> {
        if visiting.contains(target) {
            return Err(Error::TargetCycle(target.clone()));
        }
        let resolved = self.resolve(target)?;
        let DefinitionKind::Target(definition) = &resolved.definition.kind else {
            return Err(Error::NotTarget(target.clone()));
        };
        let mut bound: BTreeMap<Symbol, Symbol> = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        visiting.push(target.clone());
        for extended in abilities(&definition.extends) {
            for (ability, handler) in self.flatten(&extended, visiting)? {
                match bound.get(&ability) {
                    Some(existing) if *existing != handler => {
                        conflicts.insert(ability);
                    }
                    _ => {
                        bound.insert(ability, handler);
                    }
                }
            }
        }
        visiting.pop();
        // A binding of its own settles whatever it extends.
        for (ability, handler) in &definition.bindings {
            if let (Reference::Definition(ability), Reference::Definition(handler)) =
                (ability, handler)
            {
                conflicts.remove(ability);
                bound.insert(ability.clone(), handler.clone());
            }
        }
        match conflicts.into_iter().next() {
            Some(ability) => Err(Error::TargetConflict {
                symbol: target.clone(),
                ability,
            }),
            None => Ok(bound.into_iter().collect()),
        }
    }

    /// The root as a function: the entry's parameters, and its body the
    /// entry called under a provide for each layer of the target.
    fn root(&self, root: &Root) -> Result<Function<Reference>, Error> {
        let entry = self.resolve(&root.entry)?;
        let header = self.function_of(&root.entry, &entry.definition)?;
        let header = header.ok_or_else(|| Error::NotCompilable(root.entry.clone()))?;
        if !header.type_params.is_empty() {
            return Err(Error::GenericEntry(root.entry.clone()));
        }
        let (row, callee, params) = match &root.within {
            None => (abilities(&header.row), &root.entry, header.params.clone()),
            Some(within) => {
                let row = self.within_row(&root.entry, header.params.len(), within, &header.row)?;
                (row, within, Vec::new())
            }
        };
        // What the entry reaches: its row, and the row of every handler
        // that takes part. A binding nothing reaches is left out, so it is
        // neither compiled nor linked.
        let bound = self.bound(&root.target, row.clone())?;

        // Said here, of the entry: checking the root would say only
        // that a function nobody wrote lacks a binding.
        if let Some(ability) = row.into_iter().find(|ability| !bound.contains_key(ability)) {
            return Err(Error::Unoffered {
                symbol: root.entry.clone(),
                target: root.target.clone(),
                ability,
            });
        }

        // What a handler takes that the target lacks is reported by
        // checking.
        let rows: BTreeMap<Symbol, Vec<Symbol>> = bound
            .iter()
            .map(|(ability, (_, row))| (ability.clone(), row.clone()))
            .collect();
        // A handler is provided over the handlers its own row needs.
        let layers = layers(&rows).ok_or_else(|| Error::TargetCycle(root.target.clone()))?;
        let depth = layers.values().max().map_or(0, |deepest| deepest + 1);

        let args = match &root.within {
            None => (0..params.len())
                .map(|number| Expr::Name(Reference::Local(number)))
                .collect(),
            Some(_) => vec![Expr::Name(Reference::Definition(root.entry.clone()))],
        };
        let mut body = Expr::Call {
            callee: Box::new(Expr::Name(Reference::Definition(callee.clone()))),
            args,
        };
        for layer in (0..depth).rev() {
            let handlers = layers
                .iter()
                .filter(|(_, at)| **at == layer)
                .map(|(ability, _)| {
                    let key = Key::Ability(Reference::Definition(ability.clone()));
                    let handler = Reference::Definition(bound[ability].0.clone());
                    (key, Expr::Name(handler))
                })
                .collect();
            body = Expr::Provide {
                bundle: Box::new(Expr::Record(handlers)),
                // Nothing is bound outside a target.
                restricted: layer == 0,
                body: Box::new(body),
            };
        }
        Ok(Function {
            type_params: Vec::new(),
            params,
            // A handler that aborts to the root may end the program with
            // anything, and the host reads whatever comes back.
            ret: Type::Unknown,
            row: Vec::new(),
            body,
        })
    }

    /// What a program that hands `entry`, of `params` parameters and
    /// `row`, to `within` takes of its target: `within`'s own row, and
    /// what of `row` the function `within` takes does not take, which
    /// the entry captures where the root hands it over.
    fn within_row(
        &self,
        entry: &Symbol,
        params: usize,
        within: &Symbol,
        row: &[Reference],
    ) -> Result<Vec<Symbol>, Error> {
        let resolved = self.resolve(within)?;
        let header = self.function_of(within, &resolved.definition)?;
        let header = header.ok_or_else(|| Error::NotCompilable(within.clone()))?;
        let (handed, provided) = match header.params.as_slice() {
            [
                Param {
                    ty: Type::Function { params, row, .. },
                    ..
                },
            ] => (params.len(), abilities(row)),
            _ => return Err(Error::NotCompilable(within.clone())),
        };
        // Said here, of the entry, as a binding it lacks is.
        if params != handed {
            return Err(Error::NotHanded {
                symbol: entry.clone(),
                within: within.clone(),
                takes: handed,
            });
        }
        let mut taken = abilities(&header.row);
        taken.extend(
            abilities(row)
                .into_iter()
                .filter(|ability| !provided.contains(ability)),
        );
        Ok(taken)
    }
}
