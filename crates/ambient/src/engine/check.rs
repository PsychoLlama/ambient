//! Check: a resolved tree and the interfaces it refers to yield an open core
//! definition. See `docs/spec/definitions.md` and `docs/spec/core.md`.
//!
//! This is the engine's side of checking: gathering what the checker reads
//! and deriving the key that covers it. The checking itself is
//! [`crate::checker`].

use std::collections::BTreeSet;
use std::sync::Arc;

use super::generate::whole;
use super::{Engine, Query, Stage, memo};
use crate::checker;
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::hash::{Hash, Hasher, encode};
use crate::lower::deep;
use crate::surface::{DefinitionKind, ExternKind, Function, Reference, TypeDefinition};
use crate::symbol::{Namespace, Symbol};
use crate::types::{AbilityInfo, ContractInfo, Env, Ty, TypeInfo};

/// What dependents check against.
#[derive(Debug)]
pub struct Interface {
    /// Changes exactly when a dependent must be rechecked.
    pub digest: Hash,
}

/// The definitions a tree names, in order of first use.
pub(super) fn definitions<'a>(names: impl FnOnce(&mut dyn FnMut(&'a Reference))) -> Vec<Symbol> {
    // In the order they are met, each once.
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut seen = BTreeSet::new();
    names(&mut |reference| {
        if let Reference::Definition(symbol) = reference
            && seen.insert(symbol)
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

/// The constant a function that was checked for its value is: the type
/// it gives, and its body, which must be one of the value forms. See
/// "Values" in `docs/spec/constants.md`.
fn constant(symbol: &Symbol, checked: &Open) -> Result<Open, Error> {
    let Tree::Function(function) = &checked.tree else {
        return Err(Error::UnknownSymbol(symbol.clone()));
    };
    if !function.body.is_value_form() {
        return Err(Error::NotConstant(symbol.clone()));
    }
    let tree = Tree::Constant {
        ty: function.ret.clone(),
        value: function.body.clone(),
    };
    Ok(Open {
        tree,
        ..checked.clone()
    })
}

impl Engine<'_> {
    /// A type definition as written, and whether it can reach itself.
    fn type_info(&self, symbol: &Symbol) -> Query<TypeInfo> {
        memo(&self.type_info, symbol, || {
            let unknown = || Error::UnknownSymbol(symbol.clone());
            let (params, ty) = self.type_of(symbol)?.ok_or_else(unknown)?;

            let mut seen = BTreeSet::new();
            let mut pending = type_definitions(&ty);
            let mut recursive = false;
            while let Some(next) = pending.pop() {
                if next == *symbol {
                    recursive = true;
                    break;
                }
                if !seen.insert(next.clone()) {
                    continue;
                }
                if let Some((_, ty)) = self.type_of(&next)? {
                    pending.extend(type_definitions(&ty));
                }
            }
            Ok(Arc::new(TypeInfo {
                params,
                ty,
                recursive,
                variance: Vec::new(),
                finite: true,
            }))
        })
    }

    /// What a type definition says its name means, and how many
    /// parameters it has: the type as written, or the one its call
    /// gave, which may name definitions as a written one does. Nothing,
    /// for a definition that is no type.
    fn type_of(&self, symbol: &Symbol) -> Result<Option<(usize, Ty)>, Error> {
        Ok(match &self.resolve(symbol)?.definition.kind {
            DefinitionKind::Type(definition) => {
                Some((definition.params.len(), definition.ty.clone()))
            }
            DefinitionKind::Derived(_) => Some((0, (*self.derived(symbol)?).clone())),
            _ => None,
        })
    }

    /// Adds every type definition and ability reachable from `roots` to
    /// `env`, then puts what was added in normal form.
    pub(super) fn gather(&self, roots: Vec<Symbol>, env: &mut Env) -> Result<(), Error> {
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
                        identity: (
                            self.sources.identity(&symbol.module.package),
                            symbol.identity(),
                        ),
                        type_params: ability
                            .type_params
                            .iter()
                            .map(|p| p.bound.clone().unwrap_or(Ty::Unknown))
                            .collect(),
                        params: ability.params.iter().map(|p| p.ty.clone()).collect(),
                        ret: ability.ret.clone(),
                    };
                    env.abilities.insert(symbol, info);
                }
                Namespace::Contract if !env.contracts.contains_key(&symbol) => {
                    let resolved = self.resolve(&symbol)?;
                    let DefinitionKind::Contract(key) = &resolved.definition.kind else {
                        return Err(Error::UnknownSymbol(symbol.clone()));
                    };
                    pending.extend(type_definitions(&key.ty));
                    let package = self.sources.identity(&symbol.module.package);
                    let info = ContractInfo {
                        identity: (package, symbol.identity()),
                        ty: key.ty.clone(),
                    };
                    env.contracts.insert(symbol, info);
                }
                _ => {}
            }
        }
        env.settle();
        let keys = std::mem::take(&mut env.contracts);
        env.contracts = keys
            .into_iter()
            .map(|(symbol, info)| {
                let ty = env.normalize(&info.ty);
                (symbol, ContractInfo { ty, ..info })
            })
            .collect();
        // Signatures are read in normal form everywhere.
        let abilities = std::mem::take(&mut env.abilities);
        env.abilities = abilities
            .into_iter()
            .map(|(symbol, info)| {
                let info = AbilityInfo {
                    type_params: info.type_params.iter().map(|b| env.normalize(b)).collect(),
                    params: info.params.iter().map(|p| env.normalize(p)).collect(),
                    ret: env.normalize(&info.ret),
                    identity: info.identity,
                };
                (symbol, info)
            })
            .collect();
        Ok(())
    }

    /// Adds the digest of everything `env` holds to `key`: what checking a
    /// type or an ability reads.
    fn reachable(&self, env: &Env, key: &mut Hasher) -> Result<(), Error> {
        let named = env.types.keys().chain(env.abilities.keys());
        for reachable in named.chain(env.contracts.keys()) {
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
            let unknown = || Error::UnknownSymbol(symbol.clone());
            let digest = match &resolved.definition.kind {
                // A type's interface is the type itself, and an ability's
                // the ability.
                // An extern has nothing but a signature and a tag, and a
                // target nothing but its bindings.
                DefinitionKind::Type(_)
                | DefinitionKind::Derived(_)
                | DefinitionKind::Tag
                | DefinitionKind::Brand
                | DefinitionKind::Intrinsic
                | DefinitionKind::Ability(_)
                | DefinitionKind::Contract(_)
                | DefinitionKind::Extern(_)
                | DefinitionKind::Target(_) => self.hash(symbol)?,
                // The call of a generated module is no definition of it.
                DefinitionKind::Module(_) => return Err(unknown()),
                DefinitionKind::Function(_)
                | DefinitionKind::Generated(_)
                | DefinitionKind::Constant(_) => {
                    let function = self.function_of(symbol, &resolved.definition)?;
                    let function = &*function.ok_or_else(unknown)?;
                    let mut env = Env::default();
                    self.gather(header_definitions(function), &mut env)?;
                    let (encoding, slots) = checker::encode_header(symbol, function, env)?;
                    // A constant's type is told from that of a function
                    // that takes nothing and gives the same.
                    let constant = matches!(resolved.definition.kind, DefinitionKind::Constant(_));
                    let mut digest = Hasher::new(if constant { "constant" } else { "interface" });
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
        let open = self.check_tree(symbol)?;
        let annotations = self.annotations_of(symbol)?;
        if annotations.is_empty() {
            return Ok(open);
        }
        Ok(Arc::new(Open {
            annotations,
            ..(*open).clone()
        }))
    }

    /// A definition's tree, checked, before what it is annotated with.
    fn check_tree(&self, symbol: &Symbol) -> Query<Open> {
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
        // An ability, a contract, and an extern are told apart by where they
        // are declared, which is no part of what was resolved.
        if matches!(
            &resolved.definition.kind,
            DefinitionKind::Ability(_) | DefinitionKind::Contract(_) | DefinitionKind::Extern(_)
        ) {
            key.bytes(symbol.identity().as_bytes());
        }

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
            // A derived type is checked as the type its call gave, which
            // is what the key then covers.
            DefinitionKind::Derived(_) => {
                let ty = (*self.derived(symbol)?).clone();
                key.bytes(&encode(&ty));
                let definition = TypeDefinition {
                    params: Vec::new(),
                    ty,
                };
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_type(symbol, &definition, &env)
                })
            }
            DefinitionKind::Ability(ability) => {
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_ability(symbol, ability, &package, &env)
                })
            }
            DefinitionKind::Contract(definition) => {
                self.gather(vec![symbol.clone()], &mut env)?;
                self.reachable(&env, &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_key(symbol, definition, &package, &env)
                })
            }
            DefinitionKind::Extern(native) => {
                if let ExternKind::Function { row, .. } = &native.kind {
                    self.held_to(symbol, row)?;
                }
                let env = self.environment(used, &[], &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_extern(symbol, native, &package, &env)
                })
            }
            // Resolution leaves no tag or brand: one is a type by then.
            // And the call of a generated module is no definition of it.
            // Nothing of an intrinsic is checked: a call of one is.
            DefinitionKind::Tag
            | DefinitionKind::Brand
            | DefinitionKind::Intrinsic
            | DefinitionKind::Module(_) => Err(Error::UnknownSymbol(symbol.clone())),
            DefinitionKind::Target(_) => {
                let bindings = self.bindings(symbol)?;
                key.bytes(&encode(&bindings));
                let used = bindings
                    .iter()
                    .flat_map(|(ability, handler)| [ability.clone(), handler.clone()])
                    .collect();
                let env = self.environment(used, &[], &mut key)?;
                self.stage(Stage::Check, symbol, key.finish(), || {
                    checker::check_target(symbol, &bindings, &env)
                })
            }
            DefinitionKind::Function(function) => {
                self.held_to(symbol, &function.row)?;
                self.check_function(symbol, function, used, key, false)
            }
            // A constant's value is checked as the body of a function
            // that takes nothing and gives its type.
            DefinitionKind::Constant(function) => {
                let open = self.check_function(symbol, function, used, key, false)?;
                constant(symbol, &open).map(Arc::new)
            }
            // A generated function is checked as the function its call
            // gave, which is what the key then covers.
            DefinitionKind::Generated(_) => {
                let function = self.generated(symbol)?;
                key.bytes(&encode(&*function));
                let used = definitions(|visit| whole(&function, visit));
                self.held_to(symbol, &function.row)?;
                let checked = self.check_function(symbol, &function, used, key, false);
                // A node of it is nowhere in the text: one an error is
                // at is kept apart from a node of the call.
                if let Some(at) = self.blamed.borrow_mut().remove(symbol) {
                    self.given_at.borrow_mut().insert(symbol.clone(), at);
                }
                checked
            }
        }
    }
}
