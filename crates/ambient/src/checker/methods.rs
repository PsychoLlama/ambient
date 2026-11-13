//! Methods: fields of a record that receive the record they are read
//! from. See `spec/methods.md`.
//!
//! A record holds a method as a closure over a function the checker
//! writes: one that captures the method's function and the record, and
//! calls the first with the second. The record is filled in when the
//! field is read, so the record never holds itself.
//!
//! A merge hands each method its operands' types list the merged
//! record, and holds the merged record to what each needs
//! (`records.rs`). A method no type lists stays bound to the record it
//! had, which the runtime sees to.
//!
//! A method written in a record may leave its receiver's type out. It
//! then needs the record it is written in, as a reader sees it: each
//! method as the function reading it gives. Such a method is checked
//! after the rest of the record, and another like it is in that type
//! only if it states what it takes and gives, since its body is not
//! checked yet.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::{Ty, TyKey, abilities, generalize, instantiate};

impl Checker<'_> {
    /// Whether `value` is a method that leaves its receiver's type out.
    pub(super) fn implicit(value: &Expr<Reference>) -> bool {
        let Expr::Method(function) = value else {
            return false;
        };
        matches!(&**function, Expr::Lambda { params, .. } if matches!(params.first(), Some((_, None))))
    }

    /// The function reading a method written as `lambda` gives, if the
    /// lambda states it: the type of each parameter after the receiver,
    /// and what it gives.
    pub(super) fn stated(&mut self, lambda: &Expr<Reference>) -> Result<Option<Ty>, Error> {
        let Expr::Method(function) = lambda else {
            return Ok(None);
        };
        let Expr::Lambda {
            type_params,
            params,
            row,
            returns: Some(returns),
            ..
        } = &**function
        else {
            return Ok(None);
        };
        let mut types = vec![returns];
        for (_, ty) in params.iter().skip(1) {
            let Some(ty) = ty else {
                return Ok(None);
            };
            types.push(ty);
        }
        let mut normal = Vec::new();
        for ty in types {
            self.well_formed(ty)?;
            normal.push(self.env.normalize(ty));
        }
        let mut row = row.clone();
        row.sort();
        row.dedup();
        let function = Ty::Function {
            ret: Box::new(normal.remove(0)),
            params: normal,
            row,
        };
        // One with type parameters states a generic function type.
        let (outer, own) = (self.env.parameters(), type_params.len());
        if own == 0 {
            return Ok(Some(function));
        }
        let bounds = type_params.iter().map(|param| match &param.bound {
            Some(bound) => generalize(&self.env.normalize(bound), outer, own),
            None => Ty::Unknown,
        });
        Ok(Some(Ty::Generic {
            bounds: bounds.collect(),
            function: Box::new(generalize(&function, outer, own)),
        }))
    }

    /// A method written under a key of a record: what the record holds
    /// for it, and its type. `receiver` is the type of its receiver if
    /// it leaves that out.
    pub(super) fn method(
        &mut self,
        value: &Expr<Reference>,
        receiver: Option<&Ty>,
    ) -> Result<(core::Expr, Ty), Error> {
        let Expr::Method(function) = value else {
            return self.infer(value);
        };
        let (function, found) = match receiver {
            Some(receiver) => self.lambda(function, Some(std::slice::from_ref(receiver)))?,
            None => self.infer(function)?,
        };
        // A generic method's type parameters are the reading function's
        // too: they are in scope while it is written.
        let outer = self.env.parameters();
        let (bounds, written) = match self.env.unfold(&found) {
            Ty::Generic { bounds, function } => (bounds, *function),
            other => (Vec::new(), other),
        };
        let own: Vec<Ty> = (outer..outer + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        let open = |ty: &Ty| self.env.normalize(&instantiate(ty, &own));
        let (needs, params, ret, row) = match open(&written) {
            Ty::Function { params, ret, row } if !params.is_empty() => {
                (params[0].clone(), params[1..].to_vec(), *ret, row)
            }
            _ => return Err(self.not_method(&found)),
        };
        // What it needs of its record is asked where the record is
        // made, where its own type parameters stand for nothing.
        let bounds: Vec<Ty> = bounds.iter().map(open).collect();
        let close = |ty: &Ty| generalize(ty, outer, own.len());
        if !close(&needs).closed() {
            return Err(self.not_method(&found));
        }

        // The function reading the field gives: it takes what the
        // method takes after its record, and from its caller the same.
        // Its parameters are no locals the program wrote, so they take
        // no numbers from those that follow.
        let next = self.next;
        self.env.inner.borrow_mut().extend(bounds.iter().cloned());
        self.enter(abilities(&row));
        self.frame().generic = own.len();
        for param in &params {
            self.bind_local(param.clone());
        }
        let mut args = vec![core::Expr::Captured(1)];
        args.extend((0..params.len()).map(core::Expr::Local));
        let callee = match own.len() {
            0 => core::Expr::Captured(0),
            _ => self.instantiated(core::Expr::Captured(0), &own),
        };
        let callee = Box::new(callee);
        let body = core::Expr::Call { callee, args };
        let made = self.leave(&[found, needs.clone()], &params, &ret, body);
        self.env
            .inner
            .borrow_mut()
            .truncate(outer - self.env.bounds.len());
        self.next = next;
        let closure = made.closure(vec![function, core::Expr::Null]);

        let function = Ty::Function {
            params,
            ret: Box::new(ret),
            row,
        };
        let function = Box::new(match own.len() {
            0 => function,
            _ => Ty::Generic {
                bounds: bounds.iter().map(close).collect(),
                function: Box::new(close(&function)),
            },
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
