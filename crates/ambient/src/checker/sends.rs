//! Method calls. See `spec/methods.md`.
//!
//! What a call means is decided by the type of its receiver: a field of
//! a record, or else a function of the module its type is defined in
//! or of the module the standard library has for its shape. The
//! engine found those for every receiver the call could have
//! (`engine/environment.rs`), since only checking knows the type.

use super::Checker;
use super::calls::Checked;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, TyKey};

impl Checker<'_> {
    pub(super) fn send(
        &mut self,
        target: &Expr<Reference>,
        method: &TyKey,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (receiver, found) = self.infer(target)?;
        if self.env.field(&found, method).is_some() {
            return self.send_field(receiver, &found, method, args);
        }
        let name = Self::key_name(method);
        let receivers = self.env.receivers(&found);
        let function = receivers
            .into_iter()
            .find_map(|receiver| self.env.methods.get(&(receiver, name.clone())));
        match function.cloned() {
            Some(function) => self.send_function(&function, (receiver, found), args),
            None => Err(Error::NoMethod {
                symbol: self.symbol.clone(),
                ty: self.env.show(&found),
                name,
            }),
        }
    }

    /// A call of what a record holds under `key`. A method is called
    /// with the record, so the read that would bind the two is never
    /// made; anything else is read and then called.
    fn send_field(
        &mut self,
        record: core::Expr,
        found: &Ty,
        key: &TyKey,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let method = self.env.method(found, key).is_some();
        let (read, ty) = self.field(record, found, key)?;
        let callee = Expr::Field {
            target: Box::new(Expr::Null),
            key: key.clone(),
        };
        let (args, ret) = self.arguments(&callee, &ty, None, args)?;
        let core = match read {
            core::Expr::Field { target, key } if method => core::Expr::Send { target, key, args },
            callee => core::Expr::Call {
                callee: Box::new(callee),
                args,
            },
        };
        Ok((core, ret))
    }

    /// A call of the function a method means, with the receiver first.
    fn send_function(
        &mut self,
        function: &Symbol,
        receiver: Checked,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if let Some(signature) = self.generic(function) {
            return self.generic_call(function, &signature, Some(receiver), args);
        }
        let callee = Expr::Name(Reference::Definition(function.clone()));
        let (core_callee, ty) = self.infer(&callee)?;
        let (args, ret) = self.arguments(&callee, &ty, Some(receiver), args)?;
        let callee = Box::new(core_callee);
        Ok((core::Expr::Call { callee, args }, ret))
    }
}
