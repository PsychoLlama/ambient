//! Calls, and generic functions given their type arguments.

use super::Checker;
use super::generics::function_type;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::Ty;

/// An expression already checked: its core form and its type.
pub(super) type Checked = (core::Expr, Ty);

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
            return self.generic_call(function, &signature, None, args);
        }
        let (core_callee, ty) = self.infer(callee)?;
        let (args, ret) = self.arguments(callee, &ty, None, args)?;
        let callee = Box::new(core_callee);
        Ok((core::Expr::Call { callee, args }, ret))
    }

    /// The arguments of a call of `callee`, a value of type `ty`, each
    /// held to the parameter it is for; and what the call gives.
    /// `first` is an argument already checked, which goes before `args`.
    pub(super) fn arguments(
        &mut self,
        callee: &Expr<Reference>,
        ty: &Ty,
        first: Option<Checked>,
        args: &[Expr<Reference>],
    ) -> Result<(Vec<core::Expr>, Ty), Error> {
        let Ty::Function { params, ret, row } = self.env.unfold(ty) else {
            return Err(Error::NotCallable {
                symbol: self.symbol.clone(),
                found: self.env.show(ty),
            });
        };
        let found = args.len() + usize::from(first.is_some());
        if params.len() != found {
            return Err(match callee {
                Expr::Name(Reference::Definition(target)) => Error::Arity {
                    symbol: self.symbol.clone(),
                    callee: target.clone(),
                    expected: params.len(),
                    found,
                },
                _ => Error::NotCallable {
                    symbol: self.symbol.clone(),
                    found: self.env.show(ty),
                },
            });
        }
        self.require_all(&row)?;
        let mut params = params.iter();
        let mut core_args = Vec::new();
        if let Some((core, found)) = first
            && let Some(param) = params.next()
        {
            let (core, found) = self.bind_row(core, found, param)?;
            if !self.env.subtype(&found, param) {
                return Err(self.mismatch(param, &found));
            }
            core_args.push(core);
        }
        for (arg, param) in args.iter().zip(params) {
            core_args.push(self.check(arg, param)?);
        }
        Ok((core_args, *ret))
    }
}
