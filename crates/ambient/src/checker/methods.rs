//! Methods: fields of a record that receive the record they are read
//! from. See `spec/methods.md`.
//!
//! A record holds a method as a closure over a function the checker
//! writes: one that captures the method's function and the record, and
//! calls the first with the second. The record is filled in when the
//! field is read, so the record never holds itself.
//!
//! TODO: the receiver's type must be written. One left out should be
//! the type of the record the method is written in.
//!
//! TODO: calling a method reads it first, which makes a function. A
//! call could reach the method's function with the record directly.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::{Ty, TyKey, abilities};

impl Checker<'_> {
    /// A method written under a key of a record: what the record holds
    /// for it, and its type.
    pub(super) fn method(&mut self, function: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        let (function, found) = self.infer(function)?;
        let (needs, params, ret, row) = match self.env.unfold(&found) {
            Ty::Function { params, ret, row } if !params.is_empty() => {
                (params[0].clone(), params[1..].to_vec(), *ret, row)
            }
            _ => return Err(self.not_method(&found)),
        };

        // The function reading the field gives: it takes what the
        // method takes after its record, and from its caller the same.
        self.enter(abilities(&row));
        for param in &params {
            self.bind_local(param.clone());
        }
        let mut args = vec![core::Expr::Captured(1)];
        args.extend((0..params.len()).map(core::Expr::Local));
        let body = core::Expr::Call {
            callee: Box::new(core::Expr::Captured(0)),
            args,
        };
        let made = self.leave(&[found, needs.clone()], &params, &ret, body);
        let closure = made.closure(vec![function, core::Expr::Null]);

        let function = Box::new(Ty::Function {
            params,
            ret: Box::new(ret),
            row,
        });
        let needs = Box::new(needs);
        Ok((
            core::Expr::Method(Box::new(closure)),
            Ty::Method { needs, function },
        ))
    }

    pub(super) fn not_method(&self, found: &Ty) -> Error {
        Error::Method {
            symbol: self.symbol.clone(),
            found: self.env.show(found),
        }
    }

    /// Fails unless a record of type `record` has what each of its
    /// methods needs of it.
    pub(super) fn receives(&self, record: &Ty, fields: &[(TyKey, Ty)]) -> Result<(), Error> {
        for (key, ty) in fields {
            if let Ty::Method { needs, .. } = ty
                && !self.env.subtype(record, needs)
            {
                return Err(Error::Receiver {
                    symbol: self.symbol.clone(),
                    name: Self::key_name(key),
                    needs: self.env.show(needs),
                    found: self.env.show(record),
                });
            }
        }
        Ok(())
    }
}
