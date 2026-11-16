//! Derived types and generated functions: a definition that is
//! whatever tree a comptime call gives. See "Splice positions" in
//! `spec/comptime.md`.
//!
//! The call is checked as the body of a function the engine writes,
//! which takes nothing and gives the tree. Everything a comptime
//! call needs follows from that: its key, its grants, and its runs.

use std::sync::Arc;

use super::check::definitions;
use super::{Engine, Query, memo};
use crate::checker;
use crate::core::{self, Tree};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::{DefinitionKind, Expr, Function, Reference};
use crate::symbol::Symbol;
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
        let ty = checker::written(&tree);
        ty.map(Arc::new)
            .ok_or_else(|| Error::NotType(symbol.clone()))
    }

    fn generate(&self, symbol: &Symbol) -> Query<Function<Reference>> {
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Generated(call) = &resolved.definition.kind else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let tree = self.given(symbol, &resolved.digest, call, checker::function())?;
        let function = checker::defined(&tree).ok_or_else(|| Error::NotCode(symbol.clone()))?;
        // A tree is data, and could name what nothing written here may.
        let package = &symbol.module.package;
        let named = definitions(|visit| whole(&function, visit));
        match named
            .into_iter()
            .find(|named| !self.nameable(package, named))
        {
            Some(named) => Err(Error::Unnameable {
                symbol: symbol.clone(),
                named,
            }),
            None => Ok(Arc::new(function)),
        }
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

/// Calls `visit` with every name in a function: its header's and its
/// body's.
pub(super) fn whole<'a>(function: &'a Function<Reference>, visit: &mut dyn FnMut(&'a Reference)) {
    function.header_names(&mut |name| visit(name));
    function.body.names(&mut |name| visit(name));
}
