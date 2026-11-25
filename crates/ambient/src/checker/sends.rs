//! Method calls. See `docs/spec/methods.md`.
//!
//! What a call means is decided by the type of its receiver: a field of
//! a record, or the function the standard library has under that name
//! for a built-in shape. A value is one or the other, so nothing is
//! tried in turn. The engine found the functions for every shape
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
        // An opaque type has the methods of its module, wherever it is,
        // and no field one could be put in.
        if let Ty::Named(Reference::Definition(owner), _) = &found
            && self.env.types.get(owner).is_some_and(|info| info.opaque)
        {
            let name = Self::key_name(method);
            let function = self.env.owned.get(&(owner.clone(), name.clone()));
            let takes = |first: &Ty| matches!(first, Ty::Named(Reference::Definition(of), _) if of == owner);
            return match function
                .filter(|function| self.first_is(function, takes))
                .cloned()
            {
                Some(function) => self.send_function(&function, (receiver, found), args),
                None => Err(Error::NoMethod {
                    symbol: self.symbol.clone(),
                    ty: self.env.show(&found),
                    name,
                }),
            };
        }
        if self.env.field(&found, method).is_some() {
            return self.send_field(receiver, &found, method, args);
        }
        let name = Self::key_name(method);
        let shape = self.env.shape(&found);
        let function = shape.and_then(|shape| {
            let function = self.env.methods.get(&(shape, name.clone()))?;
            let takes = |first: &Ty| self.env.shape(first) == Some(shape);
            self.first_is(function, takes).then_some(function)
        });
        match function.cloned() {
            Some(function) => self.send_function(&function, (receiver, found), args),
            None => Err(Error::NoMethod {
                symbol: self.symbol.clone(),
                ty: self.env.show(&found),
                name,
            }),
        }
    }

    /// Whether `function` takes a first parameter, of a type `receives`
    /// accepts: a function of a module is a method only of what it
    /// takes first. See "Built-in methods" in `docs/spec/methods.md`.
    fn first_is(&self, function: &Symbol, receives: impl Fn(&Ty) -> bool) -> bool {
        let signature = self.env.functions.get(function);
        signature
            .and_then(|signature| signature.params.first())
            .is_some_and(receives)
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
        // A field that holds a generic function is read, and what is
        // read is instantiated by its arguments.
        if !method && matches!(self.env.unfold(&ty), Ty::Generic { .. }) {
            return self.generic_value_call(read, &ty, None, args);
        }
        // A generic method is called with the record too, at whatever
        // type arguments its other arguments require.
        if matches!(self.env.unfold(&ty), Ty::Generic { .. }) {
            let (signature, own) = self.opened(&ty)?;
            let solved = self.generic_arguments(None, &signature, own, None, args)?;
            let types = solved.types[own..].iter();
            let types = types.map(|ty| self.lower(ty)).collect();
            let core::Expr::Field { target, key } = read else {
                return Ok((read, solved.ret));
            };
            let args = solved.args;
            return Ok((
                core::Expr::Send {
                    target,
                    key,
                    args,
                    types,
                },
                solved.ret,
            ));
        }
        let callee = Expr::Field {
            target: Box::new(Expr::Null),
            key: key.clone(),
        };
        let (args, ret) = self.arguments(&callee, &ty, None, args)?;
        let types = Vec::new();
        let core = match read {
            core::Expr::Field { target, key } if method => core::Expr::Send {
                target,
                key,
                args,
                types,
            },
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
