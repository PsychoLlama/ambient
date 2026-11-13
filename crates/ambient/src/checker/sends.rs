//! Method calls. See `spec/methods.md`.
//!
//! What a call means is decided by the type of its receiver: a field of
//! a record, or the function of the standard library that the
//! receiver's shape has under that name. Resolution found the second
//! for every shape (`engine/scope.rs`), since it cannot know the type.

use super::Checker;
use super::calls::Checked;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference, Shape};
use crate::types::{Ty, TyKey};

impl Checker<'_> {
    pub(super) fn send(
        &mut self,
        target: &Expr<Reference>,
        method: &TyKey,
        args: &[Expr<Reference>],
        shapes: &[(Shape, Reference)],
    ) -> Result<(core::Expr, Ty), Error> {
        let (receiver, found) = self.infer(target)?;
        if self.env.field(&found, method).is_some() {
            return self.send_field(receiver, &found, method, args);
        }
        let shape = self.env.shape(&found);
        let function = shapes
            .iter()
            .find(|(has, _)| Some(*has) == shape)
            .map(|(_, function)| function);
        match function {
            Some(function) => self.send_shape(function, (receiver, found), args),
            None => Err(Error::NoMethod {
                symbol: self.symbol.clone(),
                ty: self.env.show(&found),
                name: Self::key_name(method),
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

    /// A call of the function a shape has for a method, with the
    /// receiver first.
    fn send_shape(
        &mut self,
        function: &Reference,
        receiver: Checked,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if let Reference::Definition(symbol) = function
            && let Some(signature) = self.generic(symbol)
        {
            return self.generic_call(symbol, &signature, Some(receiver), args);
        }
        let callee = Expr::Name(function.clone());
        let (core_callee, ty) = self.infer(&callee)?;
        let (args, ret) = self.arguments(&callee, &ty, Some(receiver), args)?;
        let callee = Box::new(core_callee);
        Ok((core::Expr::Call { callee, args }, ret))
    }
}
