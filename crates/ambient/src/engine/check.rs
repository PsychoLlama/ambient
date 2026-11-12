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
use crate::core::{Open, encode_signature};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::{DefinitionKind, Function, Reference};
use crate::symbol::{Namespace, Symbol};
use crate::types::{Env, Signature, Ty, TypeInfo};

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
            let DefinitionKind::Type(ty) = &resolved.definition.kind else {
                return Err(Error::UnknownSymbol(symbol.clone()));
            };

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
                if let DefinitionKind::Type(ty) = &self.resolve(&next)?.definition.kind {
                    pending.extend(type_definitions(ty));
                }
            }
            Ok(Arc::new(TypeInfo {
                ty: ty.clone(),
                recursive,
            }))
        })
    }

    /// Adds every type definition reachable from `roots` to `env`.
    fn gather(&self, roots: Vec<Symbol>, env: &mut Env) -> Result<(), Error> {
        let mut pending = roots;
        while let Some(symbol) = pending.pop() {
            if env.types.contains_key(&symbol) {
                continue;
            }
            let info = self.type_info(&symbol)?;
            pending.extend(type_definitions(&info.ty));
            env.types.insert(symbol, (*info).clone());
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
            let resolved = self.resolve(symbol)?;
            let digest = match &resolved.definition.kind {
                // A type's interface is the type itself.
                DefinitionKind::Type(_) => self.hash(symbol)?,
                DefinitionKind::Function(function) => {
                    let mut env = Env::default();
                    self.gather(header_definitions(function), &mut env)?;
                    let (params, ret, slots) = checker::lower_signature(symbol, function, &env);
                    let mut digest = Hasher::new("interface");
                    digest.bytes(&encode_signature(&params, &ret));
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

    /// A definition's open core tree.
    ///
    /// # Errors
    ///
    /// Fails if the definition does not resolve or does not check.
    pub fn check(&self, symbol: &Symbol) -> Query<Open> {
        memo(&self.check, symbol, || {
            let resolved = self.resolve(symbol)?;
            let mut env = Env::default();
            let mut key = Hasher::new("check");
            key.hash(&resolved.digest);

            match &resolved.definition.kind {
                DefinitionKind::Type(ty) => {
                    // A type refers only to types, so what checking one
                    // reads is the definitions it can reach.
                    self.gather(vec![symbol.clone()], &mut env)?;
                    for reachable in env.types.keys() {
                        key.hash(&self.resolve(reachable)?.digest);
                    }
                    self.stage(Stage::Check, symbol, key.finish(), || {
                        checker::check_type(symbol, ty, &env)
                    })
                }
                DefinitionKind::Function(function) => {
                    let mut types = Vec::new();
                    for used in definitions(|visit| {
                        resolved.definition.names(&mut |name| visit(name));
                    }) {
                        match used.namespace {
                            Namespace::Type => {
                                key.hash(&self.hash(&used)?);
                                types.push(used);
                            }
                            Namespace::Value => {
                                key.hash(&self.interface(&used)?.digest);
                                let header = self.resolve(&used)?;
                                if let DefinitionKind::Function(header) = &header.definition.kind {
                                    types.extend(header_definitions(header));
                                    let signature = Signature {
                                        params: header
                                            .params
                                            .iter()
                                            .map(|p| p.ty.clone())
                                            .collect(),
                                        ret: header.ret.clone(),
                                    };
                                    env.functions.insert(used, signature);
                                }
                            }
                            Namespace::Ability => {}
                        }
                    }
                    self.gather(types, &mut env)?;
                    // Headers are read in normal form everywhere.
                    let functions = std::mem::take(&mut env.functions);
                    env.functions = functions
                        .into_iter()
                        .map(|(symbol, signature)| {
                            let signature = Signature {
                                params: signature.params.iter().map(|p| env.normalize(p)).collect(),
                                ret: env.normalize(&signature.ret),
                            };
                            (symbol, signature)
                        })
                        .collect();
                    self.stage(Stage::Check, symbol, key.finish(), || {
                        checker::check_function(symbol, function, &env)
                    })
                }
            }
        })
    }
}
