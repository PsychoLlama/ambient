//! Check: a resolved tree and the interfaces it refers to yield an open core
//! definition. See `spec/definitions.md` and `spec/core.md`.
//!
//! This is the engine's side of checking: gathering what the checker reads
//! and deriving the key that covers it. The checking itself is
//! [`crate::checker`].

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::run::Dependency;
use super::{Engine, Query, Stage, memo};
use crate::checker;
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::hash::{Hash, Hasher, encode};
use crate::lower::deep;
use crate::surface::{DefinitionKind, ExternKind, Function, Param, Reference};
use crate::symbol::{Namespace, Symbol};
use crate::types::{AbilityInfo, Env, Signature, Ty, TypeInfo, abilities};

/// A checked function, kept with the runs its content came from.
#[derive(Serialize, Deserialize)]
struct Checked {
    result: Result<Open, Error>,
    runs: Vec<Dependency>,
}

/// What dependents check against.
#[derive(Debug)]
pub struct Interface {
    /// Changes exactly when a dependent must be rechecked.
    pub digest: Hash,
}

/// The definitions a tree names, in order of first use.
fn definitions<'a>(names: impl FnOnce(&mut dyn FnMut(&'a Reference))) -> Vec<Symbol> {
    let mut symbols: Vec<Symbol> = Vec::new();
    names(&mut |reference| {
        if let Reference::Definition(symbol) = reference
            && !symbols.contains(symbol)
        {
            symbols.push(symbol.clone());
        }
    });
    symbols
}

fn type_definitions(ty: &Ty) -> Vec<Symbol> {
    definitions(|visit| ty.names(&mut |name| visit(name)))
}

fn header_definitions(function: &Function<Reference>) -> Vec<Symbol> {
    definitions(|visit| function.header_names(&mut |name| visit(name)))
}

impl Engine<'_> {
    /// A type definition as written, and whether it can reach itself.
    fn type_info(&self, symbol: &Symbol) -> Query<TypeInfo> {
        memo(&self.type_info, symbol, || {
            let resolved = self.resolve(symbol)?;
            let DefinitionKind::Type(definition) = &resolved.definition.kind else {
                return Err(Error::UnknownSymbol(symbol.clone()));
            };
            let ty = &definition.ty;

            let mut seen = BTreeSet::new();
            let mut pending = type_definitions(ty);
            let mut recursive = false;
            while let Some(next) = pending.pop() {
                if next == *symbol {
                    recursive = true;
                    break;
                }
                if !seen.insert(next.clone()) {
                    continue;
                }
                if let DefinitionKind::Type(definition) = &self.resolve(&next)?.definition.kind {
                    pending.extend(type_definitions(&definition.ty));
                }
            }
            Ok(Arc::new(TypeInfo {
                params: definition.params.len(),
                ty: ty.clone(),
                recursive,
            }))
        })
    }

    /// Adds every type definition and ability reachable from `roots` to
    /// `env`, then puts what was added in normal form.
    fn gather(&self, roots: Vec<Symbol>, env: &mut Env) -> Result<(), Error> {
        let mut pending = roots;
        while let Some(symbol) = pending.pop() {
            match symbol.namespace {
                Namespace::Type if !env.types.contains_key(&symbol) => {
                    let info = self.type_info(&symbol)?;
                    pending.extend(type_definitions(&info.ty));
                    env.types.insert(symbol, (*info).clone());
                }
                Namespace::Ability if !env.abilities.contains_key(&symbol) => {
                    let resolved = self.resolve(&symbol)?;
                    let DefinitionKind::Ability(ability) = &resolved.definition.kind else {
                        return Err(Error::UnknownSymbol(symbol.clone()));
                    };
                    pending.extend(definitions(|visit| {
                        resolved.definition.names(&mut |name| visit(name));
                    }));
                    let info = AbilityInfo {
                        tag: (
                            self.sources.identity(&symbol.module.package),
                            ability.tag.clone(),
                        ),
                        params: ability.params.iter().map(|p| p.ty.clone()).collect(),
                        ret: ability.ret.clone(),
                    };
                    env.abilities.insert(symbol, info);
                }
                _ => {}
            }
        }
        // Signatures are read in normal form everywhere.
        let abilities = std::mem::take(&mut env.abilities);
        env.abilities = abilities
            .into_iter()
            .map(|(symbol, info)| {
                let info = AbilityInfo {
                    params: info.params.iter().map(|p| env.normalize(p)).collect(),
                    ret: env.normalize(&info.ret),
                    tag: info.tag,
                };
                (symbol, info)
            })
            .collect();
        Ok(())
    }

    /// Adds the digest of everything `env` holds to `key`: what checking a
    /// type or an ability reads.
    fn reachable(&self, env: &Env, key: &mut Hasher) -> Result<(), Error> {
        for reachable in env.types.keys().chain(env.abilities.keys()) {
            key.hash(&self.resolve(reachable)?.digest);
        }
        Ok(())
    }

    /// A definition's interface, from its header alone.
    ///
    /// # Errors
    ///
    /// Fails if the header does not resolve, or a type it names does not
    /// check.
    pub fn interface(&self, symbol: &Symbol) -> Query<Interface> {
        memo(&self.interface, symbol, || {
            // A received definition has no header apart from its tree.
            if self.received(symbol).is_some() {
                let digest = self.hash(symbol)?;
                return Ok(Arc::new(Interface { digest }));
            }
            if !symbol.lambda.is_empty() {
                return self.lambda_interface(symbol);
            }
            let resolved = self.resolve(symbol)?;
            let digest = match &resolved.definition.kind {
                // A type's interface is the type itself, and an ability's
                // the ability.
                // An extern has nothing but a signature and a tag, and a
                // target nothing but its bindings.
                DefinitionKind::Type(_)
                | DefinitionKind::Ability(_)
                | DefinitionKind::Extern(_)
                | DefinitionKind::Target(_) => self.hash(symbol)?,
                DefinitionKind::Function(function) => {
                    let mut env = Env::default();
                    self.gather(header_definitions(function), &mut env)?;
                    let (encoding, slots) = checker::encode_header(symbol, function, env)?;
                    let mut digest = Hasher::new("interface");
                    digest.bytes(&encoding);
                    // Transparent types are already expanded into the
                    // encoding; what is left are recursive ones.
                    for slot in &slots {
                        digest.hash(&self.hash(slot)?);
                    }
                    digest.finish()
                }
            };
            Ok(Arc::new(Interface { digest }))
        })
    }

    /// A lambda's interface: its header, which checking its encloser
    /// produced.
    fn lambda_interface(&self, symbol: &Symbol) -> Query<Interface> {
        let open = self.check(symbol)?;
        let mut digest = Hasher::new("interface");
        if let Tree::Function(function) = &open.tree {
            digest.bytes(&core::encode_signature(
                &function.type_params,
                &[function.captures.clone(), function.params.clone()].concat(),
                &function.ret,
                &[function.bindings.clone(), function.row.clone()].concat(),
            ));
        }
        for slot in open
            .slots
            .iter()
            .filter(|s| s.namespace != Namespace::Value)
        {
            digest.hash(&self.hash(slot)?);
        }
        Ok(Arc::new(Interface {
            digest: digest.finish(),
        }))
    }

    /// A definition's open core tree.
    ///
    /// # Errors
    ///
    /// Fails if the definition does not resolve or does not check.
    pub fn check(&self, symbol: &Symbol) -> Query<Open> {
        if let Some(open) = self.received(symbol) {
            return Ok(open);
        }
        deep(|| self.check_once(symbol))
    }

    fn check_once(&self, symbol: &Symbol) -> Query<Open> {
        memo(&self.check, symbol, || {
            if !symbol.lambda.is_empty() {
                // A lambda is checked as part of the definition it is
                // written in.
                let owner = self.check(&symbol.owner())?;
                let mut open: &Open = &owner;
                for index in &symbol.lambda {
                    open = open
                        .lambdas
                        .get(*index)
                        .ok_or_else(|| Error::UnknownSymbol(symbol.clone()))?;
                }
                return Ok(Arc::new(open.clone()));
            }
            // A comptime call inside this definition may come back here.
            if self.checking.borrow().contains(symbol) {
                return Err(Error::ComptimeCycle(symbol.clone()));
            }
            self.checking.borrow_mut().push(symbol.clone());
            let result = self.check_named(symbol);
            self.checking.borrow_mut().pop();
            // Whatever walks the tree from here on counts on this.
            match result {
                Ok(open) if !open.shallow() => Err(Error::Nesting(symbol.clone())),
                result => result,
            }
        })
    }

    fn check_named(&self, symbol: &Symbol) -> Query<Open> {
        if let Some(root) = self.root_of(symbol) {
            return self.check_root(symbol, &root);
        }
        let resolved = self.resolve(symbol)?;
        let mut env = Env::default();
        // Tags are scoped by the package's ID, which no definition holds.
        let package = self.sources.identity(&symbol.module.package);
        let mut key = Hasher::new("check");
        key.hash(&resolved.digest).bytes(package.as_bytes());
        let used = definitions(|visit| resolved.definition.names(&mut |name| visit(name)));

        match &resolved.definition.kind {
            DefinitionKind::Type(definition) => {
                // A type refers only to types, so what checking one
                // reads is the definitions it can reach.
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_type(symbol, definition, &env)
                })
            }
            DefinitionKind::Ability(ability) => {
                self.tags(&symbol.module.package)?;
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_ability(symbol, ability, &package, &env)
                })
            }
            DefinitionKind::Extern(native) => {
                self.tags(&symbol.module.package)?;
                let env = self.environment(used, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_extern(symbol, native, &package, &env)
                })
            }
            DefinitionKind::Target(_) => {
                let bindings = self.bindings(symbol)?;
                key.bytes(&encode(&bindings));
                let used = bindings
                    .iter()
                    .flat_map(|(ability, handler)| [ability.clone(), handler.clone()])
                    .collect();
                let env = self.environment(used, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_target(symbol, &bindings, &env)
                })
            }
            DefinitionKind::Function(function) => {
                self.check_function(symbol, function, used, key, false)
            }
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
        let env = self.environment(used, &mut key)?;
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
        // A comptime call that takes abilities observed the world. Its
        // result is kept with the definition and looked at again before
        // the definition is reused.
        let still = |stored: &Result<Checked, Error>| {
            stored
                .as_ref()
                .is_ok_and(|checked| self.still_gives(&checked.runs))
        };
        let checked = self.stage_if(Stage::Check, symbol, key.finish(), still, || {
            self.begin_runs();
            let result = checker::check_function(symbol, function, env, self, root);
            let runs = self.end_runs();
            Ok(Checked { result, runs })
        })?;
        checked.result.clone().map(Arc::new)
    }

    /// Everything checking reads about the definitions in `used`, with
    /// what covers it added to `key`.
    fn environment(&self, used: Vec<Symbol>, key: &mut Hasher) -> Result<Env, Error> {
        let mut env = Env::default();
        let mut roots = Vec::new();
        for used in used {
            match used.namespace {
                Namespace::Type | Namespace::Ability => {
                    key.hash(&self.hash(&used)?);
                    roots.push(used);
                }
                Namespace::Value => {
                    key.hash(&self.interface(&used)?.digest);
                    if let Some((signature, named)) = self.signature(&used)? {
                        roots.extend(named);
                        env.functions.insert(used, signature);
                    }
                }
            }
        }
        self.gather(roots, &mut env)?;
        // Headers are read in normal form everywhere.
        let functions = std::mem::take(&mut env.functions);
        env.functions = functions
            .into_iter()
            .map(|(symbol, signature)| {
                let signature = Signature {
                    type_params: signature
                        .type_params
                        .iter()
                        .map(|bound| env.normalize(bound))
                        .collect(),
                    params: signature.params.iter().map(|p| env.normalize(p)).collect(),
                    ret: env.normalize(&signature.ret),
                    ..signature
                };
                (symbol, signature)
            })
            .collect();
        Ok(env)
    }

    /// The header of a function or an extern as written, and the
    /// definitions it names. A target has none: it is not a value.
    pub(super) fn signature(
        &self,
        symbol: &Symbol,
    ) -> Result<Option<(Signature, Vec<Symbol>)>, Error> {
        let resolved = self.resolve(symbol)?;
        let named = definitions(|visit| match &resolved.definition.kind {
            DefinitionKind::Function(header) => header.header_names(&mut |name| visit(name)),
            _ => resolved.definition.names(&mut |name| visit(name)),
        });
        let types = |params: &[Param<Reference>]| params.iter().map(|p| p.ty.clone()).collect();
        let row = |row: &[Reference]| {
            let mut row = abilities(row);
            row.sort();
            row.dedup();
            row
        };
        let signature = match &resolved.definition.kind {
            DefinitionKind::Function(header) => Signature {
                type_params: header
                    .type_params
                    .iter()
                    .map(|p| p.bound.clone().unwrap_or(Ty::Unknown))
                    .collect(),
                params: types(&header.params),
                ret: header.ret.clone(),
                row: row(&header.row),
                native: false,
            },
            DefinitionKind::Extern(native) => match &native.kind {
                ExternKind::Function { params, ret } => Signature {
                    type_params: Vec::new(),
                    params: types(params),
                    ret: ret.clone(),
                    row: Vec::new(),
                    native: false,
                },
                // A native handler takes what its ability takes.
                ExternKind::Handler {
                    ability,
                    row: takes,
                } => {
                    let Reference::Definition(ability) = ability else {
                        return Ok(None);
                    };
                    let ability = self.resolve(ability)?;
                    let DefinitionKind::Ability(ability) = &ability.definition.kind else {
                        return Ok(None);
                    };
                    Signature {
                        type_params: Vec::new(),
                        params: types(&ability.params),
                        ret: ability.ret.clone(),
                        row: row(takes),
                        native: true,
                    }
                }
            },
            _ => return Ok(None),
        };
        Ok(Some((signature, named)))
    }
}
