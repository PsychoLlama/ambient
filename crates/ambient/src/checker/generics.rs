//! Generic functions: working out type arguments at a call.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{Signature, Ty, field, substitute};

impl Checker<'_> {
    /// A call to a generic function: its type arguments are whatever the
    /// arguments require.
    pub(super) fn generic_call(
        &mut self,
        function: &Symbol,
        signature: &Signature,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        if signature.params.len() != args.len() {
            return Err(Error::Arity {
                symbol: self.symbol.clone(),
                callee: function.clone(),
                expected: signature.params.len(),
                found: args.len(),
            });
        }
        // Lambdas wait until the other arguments have said what they can
        // about the type parameters, so their own parameters can be typed.
        // Each argument is still checked at the local numbers it was
        // resolved with.
        let start = self.next;
        let mut bases = Vec::new();
        let mut base = start;
        for arg in args {
            bases.push(base);
            base += arg.binders();
        }
        let mut checked: Vec<Option<(core::Expr, Ty)>> = vec![None; args.len()];
        let mut lower = vec![Vec::new(); signature.type_params.len()];
        for (position, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Lambda { .. }) {
                continue;
            }
            self.next = bases[position];
            let (core, found) = self.infer(arg)?;
            self.collect(&signature.params[position], &found, &mut lower);
            checked[position] = Some((core, found));
        }
        for (position, arg) in args.iter().enumerate() {
            if !matches!(arg, Expr::Lambda { .. }) {
                continue;
            }
            let so_far: Vec<Ty> = lower.iter().map(|b| self.env.union(b.clone())).collect();
            let expected = self
                .env
                .normalize(&substitute(&signature.params[position], &so_far));
            self.next = bases[position];
            let (core, found) = self.infer_as(arg, &expected)?;
            self.collect(&signature.params[position], &found, &mut lower);
            checked[position] = Some((core, found));
        }
        self.next = base;
        let type_args = self.solve(function, signature, lower)?;
        let mut core_args = Vec::new();
        for (param, (core, found)) in signature.params.iter().zip(checked.into_iter().flatten()) {
            let expected = self.env.normalize(&substitute(param, &type_args));
            let (core, found) = self.bind_row(core, found, &expected)?;
            if !self.env.subtype(&found, &expected) {
                return Err(self.mismatch(&expected, &found));
            }
            core_args.push(core);
        }
        for ability in &signature.row {
            self.require(ability)?;
        }
        let core = core::Expr::Call {
            callee: Box::new(self.reference(function, &type_args)),
            args: core_args,
        };
        let ret = self.env.normalize(&substitute(&signature.ret, &type_args));
        Ok((core, ret))
    }

    /// The header of `function`, if it has type parameters.
    pub(super) fn generic(&self, function: &Symbol) -> Option<Signature> {
        self.env
            .functions
            .get(function)
            .filter(|signature| !signature.type_params.is_empty())
            .cloned()
    }

    pub(super) fn reference(&mut self, function: &Symbol, type_args: &[Ty]) -> core::Expr {
        let type_args = type_args.iter().map(|arg| self.lower(arg)).collect();
        core::Expr::Reference(self.slot(function), type_args)
    }

    /// Records what `found` requires of the type parameters in `pattern`,
    /// a type from a generic header, for `found` to be a subtype of it.
    ///
    /// Only positions where a parameter must be at least some type are
    /// read. The result is verified by subtyping afterwards, so missing a
    /// requirement here costs an error message, never soundness.
    pub(super) fn collect(&self, pattern: &Ty, found: &Ty, lower: &mut [Vec<Ty>]) {
        self.collect_within(pattern, found, lower, 0);
    }

    pub(super) fn collect_within(
        &self,
        pattern: &Ty,
        found: &Ty,
        lower: &mut [Vec<Ty>],
        depth: usize,
    ) {
        // Two different recursive types can be unfolded against each other
        // forever; past this depth there is nothing new to learn.
        if depth > 32 {
            return;
        }
        let depth = depth + 1;
        // The same definition on both sides: compare arguments, without
        // unfolding either.
        if let (
            Ty::Named(Reference::Definition(name), patterns),
            Ty::Named(Reference::Definition(other), args),
        ) = (pattern, found)
            && name == other
        {
            for (pattern, arg) in patterns.iter().zip(args) {
                self.collect_within(pattern, arg, lower, depth);
            }
            return;
        }
        let found = self.env.unfold(found);
        if let Ty::Named(Reference::Definition(_), _) = pattern {
            let unfolded = self.env.unfold(pattern);
            if unfolded != *pattern {
                self.collect_within(&unfolded, &found, lower, depth);
            }
            return;
        }
        match (pattern, &found) {
            (Ty::Named(Reference::Parameter(position), _), _) => {
                if let Some(bounds) = lower.get_mut(*position) {
                    bounds.push(found);
                }
            }
            (_, Ty::Union(members)) if !matches!(pattern, Ty::Union(_)) => {
                for member in members {
                    self.collect_within(pattern, member, lower, depth);
                }
            }
            (Ty::Record(fields), Ty::Record(have)) => {
                for (name, pattern) in fields {
                    if let Some(found) = field(have, name) {
                        self.collect_within(pattern, found, lower, depth);
                    }
                }
            }
            (Ty::Tuple(patterns), Ty::Tuple(members)) => {
                for (pattern, member) in patterns.iter().zip(members) {
                    self.collect_within(pattern, member, lower, depth);
                }
            }
            (Ty::List(pattern), Ty::Tuple(members)) => {
                for member in members {
                    self.collect_within(pattern, member, lower, depth);
                }
            }
            (Ty::List(pattern), Ty::List(member)) => {
                self.collect_within(pattern, member, lower, depth);
            }
            (Ty::Function { ret: pattern, .. }, Ty::Function { ret, .. }) => {
                self.collect_within(pattern, ret, lower, depth);
            }
            // `T | null` against `int | null`: whatever the other members
            // do not already accept is what the parameter must accept.
            (Ty::Union(patterns), _) => {
                let (parameters, rest): (Vec<&Ty>, Vec<&Ty>) = patterns
                    .iter()
                    .partition(|member| matches!(member, Ty::Named(Reference::Parameter(_), _)));
                let [parameter] = parameters.as_slice() else {
                    return;
                };
                let rest = self.env.union(rest.into_iter().cloned().collect());
                let members = match &found {
                    Ty::Union(members) => members.clone(),
                    other => vec![other.clone()],
                };
                for member in members {
                    if !self.env.subtype(&member, &rest) {
                        self.collect_within(parameter, &member, lower, depth);
                    }
                }
            }
            _ => {}
        }
    }

    /// One type argument per parameter: the least type that meets what was
    /// collected.
    pub(super) fn solve(
        &self,
        function: &Symbol,
        signature: &Signature,
        lower: Vec<Vec<Ty>>,
    ) -> Result<Vec<Ty>, Error> {
        let args: Vec<Ty> = lower
            .into_iter()
            .map(|bounds| self.env.union(bounds))
            .collect();
        self.within_bounds(signature, &args)
            .map_err(|_| Error::CannotInfer {
                symbol: self.symbol.clone(),
                function: function.clone(),
            })?;
        Ok(args)
    }

    pub(super) fn within_bounds(&self, signature: &Signature, args: &[Ty]) -> Result<(), Error> {
        for (arg, bound) in args.iter().zip(&signature.type_params) {
            let bound = self.env.normalize(&substitute(bound, args));
            if !self.env.subtype(arg, &bound) {
                return Err(self.mismatch(&bound, arg));
            }
        }
        Ok(())
    }
}

/// The type of a function with this header, at these type arguments.
pub(super) fn function_type(signature: &Signature, type_args: &[Ty]) -> Ty {
    Ty::Function {
        params: signature
            .params
            .iter()
            .map(|param| substitute(param, type_args))
            .collect(),
        ret: Box::new(substitute(&signature.ret, type_args)),
        row: signature
            .row
            .iter()
            .cloned()
            .map(Reference::Definition)
            .collect(),
    }
}
