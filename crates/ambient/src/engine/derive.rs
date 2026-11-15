//! Derived types: a type that is whatever tree a comptime call gives.
//! See "Splice positions" in `spec/comptime.md`.
//!
//! The call is checked as the body of a function the engine writes,
//! which takes nothing and gives a `tree.Type`. Everything a comptime
//! call needs follows from that: its key, its grants, and its runs.

use std::sync::Arc;

use super::check::definitions;
use super::{Engine, Query, memo};
use crate::checker;
use crate::core::Tree;
use crate::error::Error;
use crate::hash::Hasher;
use crate::surface::{DefinitionKind, Function, Reference};
use crate::symbol::Symbol;
use crate::types::Ty;

impl Engine<'_> {
    /// The type the call of the derived definition `symbol` gives.
    pub(super) fn derived(&self, symbol: &Symbol) -> Query<Ty> {
        memo(&self.derived, symbol, || {
            // Checking the call reads the types it names, which may be
            // this one.
            if self.deriving.borrow().contains(symbol) {
                return Err(Error::ComptimeCycle(symbol.clone()));
            }
            self.deriving.borrow_mut().push(symbol.clone());
            let ty = self.derive(symbol);
            self.deriving.borrow_mut().pop();
            ty
        })
    }

    fn derive(&self, symbol: &Symbol) -> Query<Ty> {
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Derived(call) = &resolved.definition.kind else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let tree = checker::tree();
        let function = Function {
            type_params: Vec::new(),
            params: Vec::new(),
            ret: Ty::Named(Reference::Definition(tree.clone()), Vec::new()),
            row: Vec::new(),
            body: call.clone(),
        };
        let mut used = definitions(|visit| call.names(&mut |name| visit(name)));
        used.push(tree);
        let package = self.sources.identity(&symbol.module.package);
        let mut key = Hasher::new("derive");
        key.hash(&resolved.digest).bytes(package.as_bytes());
        let open = self.check_function(symbol, &function, used, key, false)?;
        let Tree::Function(function) = &open.tree else {
            return Err(Error::UnknownSymbol(symbol.clone()));
        };
        let ty = checker::written(&function.body);
        ty.map(Arc::new)
            .ok_or_else(|| Error::NotType(symbol.clone()))
    }
}
