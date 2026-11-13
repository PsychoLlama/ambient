//! Calls that are not of a plain function value: a method called with
//! its record, a generic function at type arguments, and a step of an
//! iterator.

use super::expr::Body;
use super::{Invalid, shape};
use crate::core::{self, Expr};
use crate::types::{Ty, instantiate};

impl Body<'_, '_> {
    /// A method called with the record that holds it. A generic one is
    /// given its type arguments.
    pub(super) fn send(&mut self, send: &Expr) -> Result<Ty, Invalid> {
        let Expr::Send {
            target,
            key,
            args,
            types,
        } = send
        else {
            return shape("a call of a method that is not one");
        };
        let found = self.infer(target)?;
        let method = self.verifier.env.method(&found, &self.key(key)?);
        let method = match method {
            Some(method) if !types.is_empty() => Some(self.applied(&method, types)?),
            method => method,
        };
        let Some(Ty::Function { params, ret, row }) = method else {
            return shape("a method is called that may be missing");
        };
        if params.len() != args.len() {
            return Err(Invalid::Arity);
        }
        self.require_all(&row)?;
        for (arg, param) in args.iter().zip(&params) {
            self.check(arg, param)?;
        }
        Ok(*ret)
    }

    /// A generic function value at type arguments: as many as its type
    /// has parameters, each within its bound.
    pub(super) fn instantiate(
        &mut self,
        function: &Expr,
        types: &[core::Type],
    ) -> Result<Ty, Invalid> {
        let found = self.infer(function)?;
        self.applied(&found, types)
    }

    /// A generic function type at type arguments.
    fn applied(&self, found: &Ty, types: &[core::Type]) -> Result<Ty, Invalid> {
        let env = &self.verifier.env;
        let Ty::Generic { bounds, function } = env.unfold(found) else {
            return shape("type arguments are given to what takes none");
        };
        if bounds.len() != types.len() {
            return Err(Invalid::Arity);
        }
        let args = self.verifier.sound_all(types)?;
        for (arg, bound) in args.iter().zip(&bounds) {
            let bound = env.normalize(&instantiate(bound, &args));
            if !env.subtype(arg, &bound) {
                return Err(self.verifier.mismatch(&bound, arg));
            }
        }
        Ok(env.normalize(&instantiate(&function, &args)))
    }

    /// A step: what an iterator gives next.
    pub(super) fn step(&mut self, iterator: &Expr) -> Result<Ty, Invalid> {
        let found = self.infer(iterator)?;
        let env = &self.verifier.env;
        let item = env.items(&found);
        item.map_or_else(
            || shape("what is stepped is not an iterator"),
            |item| Ok(env.step(&item)),
        )
    }
}
