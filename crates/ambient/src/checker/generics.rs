//! Generic functions: working out type arguments at a call.

use super::Checker;
use super::calls::Checked;
use crate::core;
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{
    Signature, Ty, Variance, abilities, entry, generalize, instantiate, substitute,
};

/// A generic call, once its type arguments are known.
pub(super) struct Solved {
    pub args: Vec<core::Expr>,
    /// A type argument for each type parameter of the header.
    pub types: Vec<Ty>,
    pub ret: Ty,
}

impl Checker<'_> {
    /// A call to a generic function: its type arguments are whatever the
    /// arguments require.
    pub(super) fn generic_call(
        &mut self,
        function: &Symbol,
        signature: &Signature,
        first: Option<Checked>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let solved = self.generic_arguments(Some(function), signature, 0, first, args)?;
        let callee = Box::new(self.reference(function, &solved.types));
        let args = solved.args;
        Ok((core::Expr::Call { callee, args }, solved.ret))
    }

    /// A call of a value of a generic function type, which is
    /// instantiated at whatever the arguments require.
    pub(super) fn generic_value_call(
        &mut self,
        callee: core::Expr,
        ty: &Ty,
        first: Option<Checked>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (signature, own) = self.opened(ty)?;
        let solved = self.generic_arguments(None, &signature, own, first, args)?;
        let callee = Box::new(self.instantiated(callee, &solved.types[own..]));
        let args = solved.args;
        Ok((core::Expr::Call { callee, args }, solved.ret))
    }

    /// A value of a generic function type at type arguments.
    pub(super) fn instantiated(&mut self, function: core::Expr, types: &[Ty]) -> core::Expr {
        core::Expr::Instantiate {
            function: Box::new(function),
            types: types.iter().map(|ty| self.lower(ty)).collect(),
        }
    }

    /// A generic function type as the header of a function to call: its
    /// parameters become type parameters numbered after those in scope,
    /// which are how many the second of the pair says.
    pub(super) fn opened(&self, ty: &Ty) -> Result<(Signature, usize), Error> {
        let not_callable = || Error::NotCallable {
            symbol: self.symbol.clone(),
            found: self.env.show(ty),
        };
        let Ty::Generic { bounds, function } = self.env.unfold(ty) else {
            return Err(not_callable());
        };
        let own = self.env.parameters();
        let fresh: Vec<Ty> = (own..own + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        let open = |ty: &Ty| self.env.normalize(&instantiate(ty, &fresh));
        let Ty::Function { params, ret, row } = open(&function) else {
            return Err(not_callable());
        };
        let outer = std::iter::repeat_n(Ty::Unknown, own);
        let signature = Signature {
            type_params: outer.chain(bounds.iter().map(open)).collect(),
            params: params.into(),
            ret: *ret,
            row: abilities(&row),
            native: false,
        };
        Ok((signature, own))
    }

    /// The arguments of a call of a generic function, its type
    /// arguments, and what it gives. `function` is the definition
    /// called, if it is one. The first `own` type parameters of
    /// `signature` are not the function's but those in scope where the
    /// call is: each stands for itself.
    pub(super) fn generic_arguments(
        &mut self,
        function: Option<&Symbol>,
        signature: &Signature,
        own: usize,
        first: Option<Checked>,
        args: &[Expr<Reference>],
    ) -> Result<Solved, Error> {
        // An argument already checked goes before the rest.
        let given = usize::from(first.is_some());
        if signature.params.len() != args.len() + given {
            return Err(match function {
                Some(function) => Error::Arity {
                    symbol: self.symbol.clone(),
                    callee: function.clone(),
                    expected: signature.params.len(),
                    found: args.len() + given,
                },
                None => Error::NotCallable {
                    symbol: self.symbol.clone(),
                    found: self.env.show(&function_type(signature, &[])),
                },
            });
        }
        let params = &signature.params[given..];
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
        let mut checked: Vec<Option<Checked>> = vec![None; args.len()];
        let mut lower = vec![Vec::new(); signature.type_params.len()];
        if let Some((_, found)) = &first {
            self.collect(&signature.params[0], found, &mut lower);
        }
        for (position, arg) in args.iter().enumerate() {
            if matches!(arg, Expr::Lambda { .. }) {
                continue;
            }
            self.next = bases[position];
            let (core, found) = self.infer(arg)?;
            self.collect(&params[position], &found, &mut lower);
            checked[position] = Some((core, found));
        }
        for (position, arg) in args.iter().enumerate() {
            if !matches!(arg, Expr::Lambda { .. }) {
                continue;
            }
            let so_far = self.least(own, lower.clone());
            let so_far = self.told(&params[position], so_far, own);
            let expected = self.env.normalize(&substitute(&params[position], &so_far));
            self.next = bases[position];
            let (core, found) = self.infer_as(arg, &expected)?;
            self.collect(&params[position], &found, &mut lower);
            checked[position] = Some((core, found));
        }
        self.next = base;
        let type_args = self.solve(function, signature, own, lower)?;
        let mut core_args = Vec::new();
        let checked = first.into_iter().chain(checked.into_iter().flatten());
        for (param, (core, found)) in signature.params.iter().zip(checked) {
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
        let ret = self.env.normalize(&substitute(&signature.ret, &type_args));
        Ok(Solved {
            args: core_args,
            types: type_args,
            ret,
        })
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
                for (key, pattern) in fields {
                    if let Some(found) = entry(have, key) {
                        self.collect_within(pattern, found, lower, depth);
                    }
                }
            }
            (Ty::Tuple(patterns), Ty::Tuple(members)) => {
                for (pattern, member) in patterns.iter().zip(members) {
                    self.collect_within(pattern, member, lower, depth);
                }
            }
            (Ty::List(pattern), Ty::List(member))
            | (Ty::Branded(pattern, _), Ty::Branded(member, _))
            | (Ty::Cell(pattern), Ty::Cell(member))
            | (Ty::Iterator(pattern), Ty::Iterator(member)) => {
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
                    // `Node<T> | null` against a node: each member says
                    // what it can, and one of another shape says nothing.
                    for member in rest {
                        self.collect_within(member, &found, lower, depth);
                    }
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

    /// The least type that meets what was collected for each type
    /// parameter. The first `own` are in scope here, and stand for
    /// themselves whatever was collected.
    fn least(&self, own: usize, lower: Vec<Vec<Ty>>) -> Vec<Ty> {
        let least = |(position, bounds): (usize, Vec<Ty>)| {
            if position < own {
                Ty::Named(Reference::Parameter(position), Vec::new())
            } else {
                self.env.union(bounds)
            }
        };
        lower.into_iter().enumerate().map(least).collect()
    }

    /// What a lambda written for `param` is told each type parameter
    /// is. One it gives back as well as takes is told at its primitive
    /// where only literals were collected: what the lambda gives may
    /// widen it, and the lambda's own parameters are typed before that
    /// is known.
    fn told(&self, param: &Ty, mut so_far: Vec<Ty>, own: usize) -> Vec<Ty> {
        let Ty::Function { ret, .. } = param else {
            return so_far;
        };
        let stands = |position| Ty::Named(Reference::Parameter(position), Vec::new());
        let own_names: Vec<Ty> = (0..so_far.len()).map(stands).collect();
        for position in own..so_far.len() {
            let mut other = own_names.clone();
            other[position] = Ty::Unknown;
            if substitute(ret, &own_names) != substitute(ret, &other) {
                so_far[position] = self.widened(&so_far[position]);
            }
        }
        so_far
    }

    /// `ty` with each literal type as its primitive.
    pub(super) fn widened(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Literal(Literal::Bool(_)) => Ty::Bool,
            Ty::Literal(Literal::Int(_)) => Ty::Int,
            Ty::Literal(Literal::Float(_)) => Ty::Float,
            Ty::Literal(Literal::String(_)) => Ty::String,
            Ty::Literal(Literal::Bytes(_)) => Ty::Bytes,
            Ty::Union(members) => {
                let members = members.iter().map(|member| self.widened(member));
                self.env.union(members.collect())
            }
            other => other.clone(),
        }
    }

    /// One type argument per parameter: the least type that meets what
    /// was collected, as [`Self::least`] has it. `function` is the
    /// definition being called, if it is one.
    pub(super) fn solve(
        &self,
        function: Option<&Symbol>,
        signature: &Signature,
        own: usize,
        lower: Vec<Vec<Ty>>,
    ) -> Result<Vec<Ty>, Error> {
        let mut args = self.least(own, lower);
        // A literal type is no use where the result holds the parameter
        // to more than being read: a map of `1 | 2` takes no `3`.
        let held = self.env.variance(&signature.ret, args.len());
        for (arg, held) in args.iter_mut().zip(held).skip(own) {
            if matches!(held, Variance::Invariant | Variance::Contravariant) {
                *arg = self.widened(arg);
            }
        }
        match (self.within_bounds(signature, &args), function) {
            (Ok(()), _) => Ok(args),
            (Err(_), Some(function)) => Err(Error::CannotInfer {
                symbol: self.symbol.clone(),
                function: function.clone(),
            }),
            (Err(error), None) => Err(error),
        }
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

/// The type of a function with this header as a value: a generic
/// function type if it has type parameters.
pub(crate) fn generic_type(signature: &Signature) -> Ty {
    let count = signature.type_params.len();
    let own: Vec<Ty> = (0..count)
        .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
        .collect();
    let function = function_type(signature, &own);
    if count == 0 {
        return function;
    }
    Ty::Generic {
        bounds: signature
            .type_params
            .iter()
            .map(|bound| generalize(bound, 0, count))
            .collect(),
        function: Box::new(generalize(&function, 0, count)),
    }
}

/// The type of a function with this header, at these type arguments.
pub(crate) fn function_type(signature: &Signature, type_args: &[Ty]) -> Ty {
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
