//! Definitions that are whatever tree a comptime call gives: a derived
//! type, a generated function, and the definitions of a generated
//! module. See "Splice positions" in `spec/comptime.md`.
//!
//! The call is checked as the body of a function the engine writes,
//! which takes nothing and gives the tree. Everything a comptime
//! call needs follows from that: its key, its grants, and its runs.
//! The tree is then read into what resolution leaves of a definition
//! that was written, and is that from there on.

use std::ops::Deref;
use std::sync::Arc;

use super::check::definitions;
use super::{Engine, Query, memo};
use crate::checker;
use crate::core::{self, Tree};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::{Definition, DefinitionKind, Expr, Function, GENERATOR, Reference};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::types::Ty;

impl Engine<'_> {
    /// The type the call of the derived definition `symbol` gives.
    pub(super) fn derived(&self, symbol: &Symbol) -> Query<Ty> {
        memo(&self.derived, symbol, || self.once(symbol, Self::derive))
    }

    /// The function the call of the generated definition `symbol`
    /// gives, resolved as one written there would be.
    pub(super) fn generated(&self, symbol: &Symbol) -> Query<Function<Reference>> {
        memo(&self.generated, symbol, || {
            self.once(symbol, Self::generate)
        })
    }

    /// The definitions the call of a generated module gives, each
    /// resolved as one written there would be; none, for a module that
    /// was written. See "Splice positions" in `spec/comptime.md`.
    pub(super) fn made(&self, module: &ModuleId) -> Query<Vec<Definition<Reference>>> {
        memo(&self.made, module, || {
            if !self.generates(module)? {
                return Ok(Arc::default());
            }
            self.once(&module.value(GENERATOR), Self::make)
        })
    }

    /// The definitions of a generated module, by symbol: what there is
    /// to check of one. A module that was written has none.
    ///
    /// # Errors
    ///
    /// Fails if the module's call does, or gives no definitions.
    pub fn generated_in(&self, module: &ModuleId) -> Result<Vec<Symbol>, Error> {
        let symbol =
            |made: &Definition<Reference>| module.symbol(made.kind.namespace(), &made.name);
        Ok(self.made(module)?.iter().map(symbol).collect())
    }

    fn make(&self, symbol: &Symbol) -> Query<Vec<Definition<Reference>>> {
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Module(call) = &resolved.definition.kind else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let tree = self.given(symbol, &resolved.digest, call, checker::module())?;
        let mut made =
            checker::definitions(&tree, &symbol.module).map_err(|why| not_code(symbol, why))?;
        let mut seen = std::collections::BTreeSet::new();
        let id = self.sources.identity(&symbol.module.package);
        for definition in &mut made {
            // A tag or a brand is named as a type and as a value, and is
            // a type from here on, as one that was written is once
            // resolved.
            let brand = matches!(definition.kind, DefinitionKind::Brand);
            let tag = brand || matches!(definition.kind, DefinitionKind::Tag);
            let twice = !seen.insert((definition.kind.namespace(), definition.name.clone()))
                || (tag && !seen.insert((Namespace::Value, definition.name.clone())));
            if twice {
                return Err(Error::GeneratedTwice {
                    symbol: symbol.clone(),
                    name: definition.name.clone(),
                });
            }
            let declared = symbol.module.ty(&definition.name);
            if brand {
                definition.kind = super::resolve::brand(&id, &declared);
            } else if tag {
                definition.kind = super::resolve::tag(&id, &declared);
            }
        }
        let named = definitions(|visit| {
            for definition in &made {
                definition.names(&mut |name| visit(name));
            }
        });
        self.names_only(symbol, named)?;
        Ok(Arc::new(made))
    }

    /// Fails unless everything the tree of `symbol` names could be
    /// named by code written in its package. A tree is data, and a
    /// function could make up a reference to what nothing written
    /// there may reach.
    fn names_only(&self, symbol: &Symbol, named: Vec<Symbol>) -> Result<(), Error> {
        let package = &symbol.module.package;
        let hidden = named
            .into_iter()
            .find(|named| !self.nameable(package, named));
        hidden.map_or(Ok(()), |named| {
            let symbol = symbol.clone();
            Err(Error::Unnameable { symbol, named })
        })
    }

    /// The function a definition is, if it is one: as written, or as
    /// its call gave it.
    pub(super) fn function_of<'a>(
        &self,
        symbol: &Symbol,
        definition: &'a Definition<Reference>,
    ) -> Result<Option<Told<'a>>, Error> {
        Ok(match &definition.kind {
            DefinitionKind::Function(function) => Some(Told::Written(function)),
            DefinitionKind::Generated(_) => Some(Told::Given(self.generated(symbol)?)),
            _ => None,
        })
    }

    /// What `run` gives for `symbol`, unless making it came back here:
    /// checking a call reads the definitions it names, which may be
    /// this one.
    fn once<T>(&self, symbol: &Symbol, run: fn(&Self, &Symbol) -> Query<T>) -> Query<T> {
        if self.deriving.borrow().contains(symbol) {
            return Err(Error::ComptimeCycle(symbol.clone()));
        }
        self.deriving.borrow_mut().push(symbol.clone());
        let made = run(self, symbol);
        self.deriving.borrow_mut().pop();
        made
    }

    fn derive(&self, symbol: &Symbol) -> Query<Ty> {
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Derived(call) = &resolved.definition.kind else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let tree = self.given(symbol, &resolved.digest, call, checker::tree())?;
        let ty = checker::closed(&tree);
        let symbol = symbol.clone();
        ty.map(Arc::new)
            .map_err(|why| Error::NotType { symbol, why })
    }

    fn generate(&self, symbol: &Symbol) -> Query<Function<Reference>> {
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Generated(call) = &resolved.definition.kind else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let tree = self.given(symbol, &resolved.digest, call, checker::function())?;
        let function = checker::defined(&tree).map_err(|why| not_code(symbol, why))?;
        self.names_only(symbol, definitions(|visit| whole(&function, visit)))?;
        Ok(Arc::new(function))
    }

    /// The tree `call`, the content of the definition `symbol`, gives:
    /// the literal a function that takes nothing and gives a `gives`
    /// has for a body, once the call is its body and it is checked.
    fn given(
        &self,
        symbol: &Symbol,
        digest: &Hash,
        call: &Expr<Reference>,
        gives: Symbol,
    ) -> Result<core::Expr, Error> {
        let function = Function {
            type_params: Vec::new(),
            params: Vec::new(),
            ret: Ty::Named(Reference::Definition(gives.clone()), Vec::new()),
            row: Vec::new(),
            body: call.clone(),
        };
        let mut used = definitions(|visit| call.names(&mut |name| visit(name)));
        used.push(gives);
        let package = self.sources.identity(&symbol.module.package);
        let mut key = Hasher::new("derive");
        key.hash(digest).bytes(package.as_bytes());
        let open = self.check_function(symbol, &function, used, key, false);
        // Where things are in the call's result is nothing a trace of
        // the definition reads.
        self.origins.borrow_mut().remove(symbol);
        match &open?.tree {
            Tree::Function(function) => Ok(function.body.clone()),
            _ => Err(Error::UnknownSymbol(symbol.clone())),
        }
    }
}

/// A function as it was written, or as a call gave it.
pub(super) enum Told<'a> {
    Written(&'a Function<Reference>),
    Given(Arc<Function<Reference>>),
}

impl Deref for Told<'_> {
    type Target = Function<Reference>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Written(function) => function,
            Self::Given(function) => function,
        }
    }
}

/// Calls `visit` with every name in a function: its header's and its
/// body's.
pub(super) fn whole<'a>(function: &'a Function<Reference>, visit: &mut dyn FnMut(&'a Reference)) {
    function.header_names(&mut |name| visit(name));
    function.body.names(&mut |name| visit(name));
}

/// The error of a tree that stands for no code, for the reason `why`.
fn not_code(symbol: &Symbol, why: String) -> Error {
    let symbol = symbol.clone();
    Error::NotCode { symbol, why }
}
