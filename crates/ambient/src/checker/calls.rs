//! Calls, and generic functions given their type arguments.

use super::Checker;
use super::generics::function_type;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// A generic function at type arguments that were written out.
    pub(super) fn instantiate(
        &mut self,
        function: &Reference,
        args: &[Ty],
    ) -> Result<(core::Expr, Ty), Error> {
        let Reference::Definition(function) = function else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let Some(signature) = self.env.functions.get(function) else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        if args.len() != signature.type_params.len() {
            return Err(Error::TypeArguments {
                symbol: self.symbol.clone(),
                expected: signature.type_params.len(),
                found: args.len(),
            });
        }
        let mut normal = Vec::new();
        for arg in args {
            self.well_formed(arg)?;
            normal.push(self.env.normalize(arg));
        }
        self.within_bounds(signature, &normal)?;
        Ok((
            self.reference(function, &normal),
            function_type(signature, &normal),
        ))
    }

    pub(super) fn call(
        &mut self,
        callee: &Expr<Reference>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if let Expr::Name(Reference::Definition(function)) = callee
            && let Some(signature) = self.generic(function)
        {
            return self.generic_call(function, &signature, args);
        }
        let (core_callee, ty) = self.infer(callee)?;
        let Ty::Function { params, ret, row } = self.env.unfold(&ty) else {
            return Err(Error::NotCallable {
                symbol: self.symbol.clone(),
                found: self.env.show(&ty),
            });
        };
        if params.len() != args.len() {
            return Err(match callee {
                Expr::Name(Reference::Definition(target)) => Error::Arity {
                    symbol: self.symbol.clone(),
                    callee: target.clone(),
                    expected: params.len(),
                    found: args.len(),
                },
                _ => Error::NotCallable {
                    symbol: self.symbol.clone(),
                    found: self.env.show(&ty),
                },
            });
        }
        self.require_all(&row)?;
        let mut core_args = Vec::new();
        for (arg, param) in args.iter().zip(&params) {
            core_args.push(self.check(arg, param)?);
        }
        let core = core::Expr::Call {
            callee: Box::new(core_callee),
            args: core_args,
        };
        Ok((core, *ret))
    }
}
