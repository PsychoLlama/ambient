//! Check: a resolved tree and the interfaces it refers to yield an open core
//! definition. See `spec/definitions.md` and `spec/core.md`.
//!
//! This is the engine's side of checking: gathering what the checker reads
//! and deriving the key that covers it. The checking itself is
//! [`crate::checker`].

use std::collections::BTreeSet;
use std::sync::Arc;

use super::{Engine, Query, Stage, memo};
use crate::checker;
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::{DefinitionKind, Function, Reference};
use crate::symbol::{Namespace, Symbol};
use crate::types::{AbilityInfo, Env, Signature, Ty, TypeInfo, abilities};

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
                        tag: (symbol.module.package.0.clone(), ability.tag.clone()),
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
            if !symbol.lambda.is_empty() {
                return self.lambda_interface(symbol);
            }
            let resolved = self.resolve(symbol)?;
            let digest = match &resolved.definition.kind {
                // A type's interface is the type itself, and an ability's
                // the ability.
                DefinitionKind::Type(_) | DefinitionKind::Ability(_) => self.hash(symbol)?,
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
            result
        })
    }

    fn check_named(&self, symbol: &Symbol) -> Query<Open> {
        let resolved = self.resolve(symbol)?;
        let mut env = Env::default();
        let mut key = Hasher::new("check");
        key.hash(&resolved.digest);

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
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_ability(symbol, ability, &env)
                })
            }
            DefinitionKind::Function(function) => {
                let mut types = Vec::new();
                for used in definitions(|visit| {
                    resolved.definition.names(&mut |name| visit(name));
                }) {
                    match used.namespace {
                        Namespace::Type | Namespace::Ability => {
                            key.hash(&self.hash(&used)?);
                            types.push(used);
                        }
                        Namespace::Value => {
                            key.hash(&self.interface(&used)?.digest);
                            let header = self.resolve(&used)?;
                            if let DefinitionKind::Function(header) = &header.definition.kind {
                                types.extend(header_definitions(header));
                                let signature = Signature {
                                    type_params: header
                                        .type_params
                                        .iter()
                                        .map(|p| p.bound.clone().unwrap_or(Ty::Unknown))
                                        .collect(),
                                    params: header.params.iter().map(|p| p.ty.clone()).collect(),
                                    ret: header.ret.clone(),
                                    row: {
                                        let mut row = abilities(&header.row);
                                        row.sort();
                                        row.dedup();
                                        row
                                    },
                                };
                                env.functions.insert(used, signature);
                            }
                        }
                    }
                }
                self.gather(types, &mut env)?;
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
                            row: signature.row,
                        };
                        (symbol, signature)
                    })
                    .collect();
                // What a comptime call returns is content of this
                // definition, and depends on everything its callee is.
                let mut callees = Vec::new();
                function.body.comptime_callees(&mut |name| {
                    if let Reference::Definition(callee) = name {
                        callees.push(callee.clone());
                    }
                });
                for callee in &callees {
                    key.hash(&self.hash(callee)?);
                }
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_function(symbol, function, env, self)
                })
            }
        }
    }
}
