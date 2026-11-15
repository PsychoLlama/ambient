//! Calls, and generic functions given their type arguments.

use super::Checker;
use super::generics::function_type;
use crate::core;
use crate::error::Error;
use crate::literal::Brand;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
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
        // Anything but a definition is a value of a generic function
        // type, if it is one.
        let Reference::Definition(function) = function else {
            let value = self.infer(&Expr::Name(function.clone()))?;
            return self.instantiate_value(value, args);
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

    /// The brand a name is, if what it names is one: a type that is the
    /// brand told apart by where that very definition is declared.
    fn brand(&self, symbol: &Symbol) -> Option<Brand> {
        match &self.env.types.get(symbol)?.ty {
            Ty::Brand(brand) if brand.brand == symbol.identity() => Some(brand.clone()),
            _ => None,
        }
    }

    /// A call of a brand: its one argument, at its type and the brand.
    /// Only the module that declares a brand brands with it.
    fn branding(
        &mut self,
        named: &Symbol,
        brand: Brand,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if named.module != self.symbol.module {
            return Err(Error::Branding {
                symbol: self.symbol.clone(),
                brand: named.clone(),
            });
        }
        let [value] = args else {
            return Err(Error::Arity {
                symbol: self.symbol.clone(),
                callee: named.clone(),
                expected: 1,
                found: args.len(),
            });
        };
        let (value, ty) = self.infer(value)?;
        let under = Box::new(self.widened(&ty));
        let ty = self
            .env
            .normalize(&Ty::Branded(under, vec![Ty::Brand(brand)]));
        let core = core::Expr::Brand {
            value: Box::new(value),
            ty: self.lower(&ty),
        };
        Ok((core, ty))
    }

    pub(super) fn call(
        &mut self,
        callee: &Expr<Reference>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if let Expr::Name(Reference::Definition(named)) = callee
            && let Some(brand) = self.brand(named)
        {
            return self.branding(named, brand, args);
        }
        if let Expr::Name(Reference::Definition(function)) = callee
            && let Some(signature) = self.generic(function)
        {
            return self.generic_call(function, &signature, None, args);
        }
        let (core_callee, ty) = self.infer(callee)?;
        if matches!(self.env.unfold(&ty), Ty::Generic { .. }) {
            return self.generic_value_call(core_callee, &ty, None, args);
        }
        let (args, ret) = self.arguments(callee, &ty, None, args)?;
        let callee = Box::new(core_callee);
        Ok((core::Expr::Call { callee, args }, ret))
    }

    /// A value of a generic function type where a function type that is
    /// not generic is expected, with these parameters: it is
    /// instantiated at whatever they require.
    pub(super) fn coerced(
        &mut self,
        (core, found): Checked,
        params: &[Ty],
    ) -> Result<(core::Expr, Ty), Error> {
        let (signature, own) = self.opened(&found)?;
        if signature.params.len() != params.len() {
            return Ok((core, found));
        }
        let mut lower = vec![Vec::new(); signature.type_params.len()];
        for (param, wanted) in signature.params.iter().zip(params) {
            self.collect(param, wanted, &mut lower);
        }
        let args = self.solve(None, &signature, own, lower)?;
        let ty = self.env.normalize(&function_type(&signature, &args));
        Ok((self.instantiated(core, &args[own..]), ty))
    }

    /// A value of a generic function type at type arguments that were
    /// written out.
    fn instantiate_value(
        &mut self,
        (core, found): Checked,
        args: &[Ty],
    ) -> Result<(core::Expr, Ty), Error> {
        let (signature, own) = self.opened(&found)?;
        let wanted = signature.type_params.len() - own;
        if args.len() != wanted {
            return Err(Error::TypeArguments {
                symbol: self.symbol.clone(),
                expected: wanted,
                found: args.len(),
            });
        }
        let scope = (0..own).map(|position| Ty::Named(Reference::Parameter(position), Vec::new()));
        let mut all: Vec<Ty> = scope.collect();
        for arg in args {
            self.well_formed(arg)?;
            all.push(self.env.normalize(arg));
        }
        self.within_bounds(&signature, &all)?;
        let ty = self.env.normalize(&function_type(&signature, &all));
        Ok((self.instantiated(core, &all[own..]), ty))
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
