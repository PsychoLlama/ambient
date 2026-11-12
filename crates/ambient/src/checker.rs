//! The type checker: a resolved definition and an [`Env`] yield an open core
//! definition. See `spec/types.md` and `spec/core.md`.
//!
//! Checking is a pure function of its arguments. Everything it knows about
//! other definitions is in the `Env`, which is what lets the engine key its
//! result by content.

use crate::core::{self, Open, Primitive, Slot, Tree};
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Arm, Expr, Function, Operator, Pattern, Reference, TypeDefinition};
use crate::symbol::Symbol;
use crate::types::{Env, Signature, Ty, field, substitute};

type Pat = Pattern<Reference>;

/// Checks a function against its own header.
///
/// # Errors
///
/// Returns the first type error in the function.
pub fn check_function(
    symbol: &Symbol,
    function: &Function<Reference>,
    mut env: Env,
) -> Result<Open, Error> {
    let header = header(symbol, function, &mut env)?;
    let mut checker = Checker::new(symbol, &env);
    let type_params = env.bounds.iter().map(|ty| checker.lower(ty)).collect();
    let params = header.params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&header.ret);
    checker.locals = header.params;
    let body = checker.check(&function.body, &header.ret)?;
    Ok(Open {
        tree: Tree::Function(core::Function {
            type_params,
            params,
            ret,
            body,
        }),
        slots: checker.slots,
    })
}

/// Checks a type definition.
///
/// # Errors
///
/// Fails if the type is ill-formed: recursive without passing through a
/// constructor, or applying itself to anything but its own parameters.
pub fn check_type(
    symbol: &Symbol,
    definition: &TypeDefinition<Reference>,
    env: &Env,
) -> Result<Open, Error> {
    contractive(symbol, &definition.ty, env, &mut vec![symbol.clone()])?;
    let mut checker = Checker::new(symbol, env);
    checker.well_formed(&definition.ty)?;
    regular(symbol, &definition.ty, definition.params.len())?;
    let ty = checker.lower(&env.normalize(&definition.ty));
    Ok(Open {
        tree: Tree::Type {
            params: definition.params.len(),
            ty,
        },
        slots: checker.slots,
    })
}

/// The canonical encoding of a function's header, and the slots it uses.
///
/// # Errors
///
/// Fails if a type in the header is ill-formed.
pub fn encode_header(
    symbol: &Symbol,
    function: &Function<Reference>,
    mut env: Env,
) -> Result<(Vec<u8>, Vec<Symbol>), Error> {
    let header = header(symbol, function, &mut env)?;
    let mut checker = Checker::new(symbol, &env);
    let type_params: Vec<_> = env.bounds.iter().map(|ty| checker.lower(ty)).collect();
    let params: Vec<_> = header.params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&header.ret);
    let encoding = core::encode_signature(&type_params, &params, &ret);
    Ok((encoding, checker.slots))
}

struct Header {
    params: Vec<Ty>,
    ret: Ty,
}

/// Validates and normalizes a header, and records its bounds in `env`.
fn header(symbol: &Symbol, function: &Function<Reference>, env: &mut Env) -> Result<Header, Error> {
    let checker = Checker::new(symbol, env);
    let mut bounds = Vec::new();
    for param in &function.type_params {
        bounds.push(match &param.bound {
            Some(bound) => {
                checker.well_formed(bound)?;
                env.normalize(bound)
            }
            None => Ty::Unknown,
        });
    }
    let mut params = Vec::new();
    for param in &function.params {
        checker.well_formed(&param.ty)?;
        params.push(env.normalize(&param.ty));
    }
    checker.well_formed(&function.ret)?;
    let ret = env.normalize(&function.ret);
    env.bounds = bounds;
    Ok(Header { params, ret })
}

/// Fails unless every use of `symbol` inside its own definition applies it
/// to exactly its own parameters, in order.
fn regular(symbol: &Symbol, ty: &Ty, params: usize) -> Result<(), Error> {
    let mut result = Ok(());
    visit_types(ty, &mut |ty| {
        if let Ty::Named(Reference::Definition(name), args) = ty
            && name == symbol
        {
            let own = args.len() == params
                && args.iter().enumerate().all(|(position, arg)| {
                    matches!(arg, Ty::Named(Reference::Parameter(p), _) if *p == position)
                });
            if !own {
                result = Err(Error::IrregularType(symbol.clone()));
            }
        }
    });
    result
}

/// Calls `visit` with `ty` and every type inside it.
fn visit_types(ty: &Ty, visit: &mut impl FnMut(&Ty)) {
    visit(ty);
    match ty {
        Ty::Record(fields) => fields.iter().for_each(|(_, ty)| visit_types(ty, visit)),
        Ty::Tuple(members) | Ty::Union(members) | Ty::Named(_, members) => {
            for member in members {
                visit_types(member, visit);
            }
        }
        Ty::List(member) => visit_types(member, visit),
        Ty::Function { params, ret } => {
            for param in params {
                visit_types(param, visit);
            }
            visit_types(ret, visit);
        }
        _ => {}
    }
}

/// Fails if following names from `ty`, without passing through a
/// constructor, leads back to a name already being followed.
fn contractive(
    symbol: &Symbol,
    ty: &Ty,
    env: &Env,
    following: &mut Vec<Symbol>,
) -> Result<(), Error> {
    match ty {
        Ty::Named(Reference::Definition(name), _) => {
            if following.contains(name) {
                return Err(Error::Uncontractive(symbol.clone()));
            }
            let Some(info) = env.types.get(name) else {
                return Ok(());
            };
            following.push(name.clone());
            let result = contractive(symbol, &info.ty, env, following);
            following.pop();
            result
        }
        Ty::Union(members) => members
            .iter()
            .try_for_each(|member| contractive(symbol, member, env, following)),
        _ => Ok(()),
    }
}

struct Checker<'a> {
    symbol: &'a Symbol,
    env: &'a Env,
    slots: Vec<Symbol>,
    /// The type of every local bound so far, by number.
    locals: Vec<Ty>,
}

impl<'a> Checker<'a> {
    fn new(symbol: &'a Symbol, env: &'a Env) -> Self {
        Self {
            symbol,
            env,
            slots: Vec::new(),
            locals: Vec::new(),
        }
    }

    fn slot(&mut self, symbol: &Symbol) -> Slot {
        let index = self
            .slots
            .iter()
            .position(|slot| slot == symbol)
            .unwrap_or_else(|| {
                self.slots.push(symbol.clone());
                self.slots.len() - 1
            });
        Slot(index)
    }

    /// A normalized type as a core type.
    fn lower(&mut self, ty: &Ty) -> core::Type {
        match ty {
            Ty::Never => core::Type::Never,
            Ty::Unknown => core::Type::Unknown,
            // Resolution never produces a local in a type.
            Ty::Null | Ty::Named(Reference::Local(_), _) => core::Type::Null,
            Ty::Named(Reference::Parameter(position), _) => core::Type::Parameter(*position),
            Ty::Bool => core::Type::Bool,
            Ty::Int => core::Type::Int,
            Ty::Float => core::Type::Float,
            Ty::String => core::Type::String,
            Ty::Literal(literal) => core::Type::Literal(literal.clone()),
            Ty::Record(fields) => core::Type::Record(
                fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.lower(ty)))
                    .collect(),
            ),
            Ty::Tuple(members) => {
                core::Type::Tuple(members.iter().map(|m| self.lower(m)).collect())
            }
            Ty::List(member) => core::Type::List(Box::new(self.lower(member))),
            Ty::Function { params, ret } => core::Type::Function {
                params: params.iter().map(|p| self.lower(p)).collect(),
                ret: Box::new(self.lower(ret)),
            },
            Ty::Union(members) => {
                let mut members: Vec<_> = members.iter().map(|m| self.lower(m)).collect();
                members.sort_by_key(core::Type::encode);
                members.dedup();
                core::Type::Union(members)
            }
            Ty::Named(Reference::Definition(symbol), args) => {
                let args = args.iter().map(|arg| self.lower(arg)).collect();
                core::Type::Reference(self.slot(symbol), args)
            }
        }
    }

    fn mismatch(&self, expected: &Ty, found: &Ty) -> Error {
        Error::Mismatch {
            symbol: self.symbol.clone(),
            expected: self.env.show(expected),
            found: self.env.show(found),
        }
    }

    /// Checks that `expr` has a subtype of `expected`.
    fn check(&mut self, expr: &Expr<Reference>, expected: &Ty) -> Result<core::Expr, Error> {
        // A generic function used as a value takes its type arguments from
        // the function type expected of it.
        if let Expr::Name(Reference::Definition(function)) = expr
            && let Some(signature) = self.generic(function)
            && let Ty::Function { params, .. } = self.env.unfold(expected)
            && params.len() == signature.params.len()
        {
            let mut lower = vec![Vec::new(); signature.type_params.len()];
            for (param, wanted) in signature.params.iter().zip(&params) {
                self.collect(param, wanted, &mut lower);
            }
            let args = self.solve(function, &signature, lower)?;
            let found = function_type(&signature, &args);
            if !self.env.subtype(&found, expected) {
                return Err(self.mismatch(expected, &found));
            }
            return Ok(self.reference(function, &args));
        }
        let (core, found) = self.infer(expr)?;
        if self.env.subtype(&found, expected) {
            Ok(core)
        } else {
            Err(self.mismatch(expected, &found))
        }
    }

    fn infer_all(
        &mut self,
        exprs: &[Expr<Reference>],
    ) -> Result<(Vec<core::Expr>, Vec<Ty>), Error> {
        let mut cores = Vec::new();
        let mut types = Vec::new();
        for expr in exprs {
            let (core, ty) = self.infer(expr)?;
            cores.push(core);
            types.push(ty);
        }
        Ok((cores, types))
    }

    /// The core form of `expr` and its type, normalized.
    fn infer(&mut self, expr: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        Ok(match expr {
            Expr::Null => (core::Expr::Null, Ty::Null),
            // A literal has its own type, and only subtyping widens it.
            Expr::Literal(literal) => (
                core::Expr::Literal(literal.clone()),
                Ty::Literal(literal.clone()),
            ),
            Expr::Name(Reference::Local(number)) => {
                let ty = self.locals.get(*number).cloned().unwrap_or(Ty::Never);
                (core::Expr::Local(*number), ty)
            }
            // Resolution never produces a type parameter as a value.
            Expr::Name(Reference::Parameter(_)) => (core::Expr::Null, Ty::Never),
            Expr::Name(Reference::Definition(symbol)) => {
                if self.generic(symbol).is_some() {
                    return Err(Error::CannotInfer {
                        symbol: self.symbol.clone(),
                        function: symbol.clone(),
                    });
                }
                let ty = match self.env.functions.get(symbol) {
                    Some(signature) => function_type(signature, &[]),
                    None => Ty::Never,
                };
                (self.reference(symbol, &[]), ty)
            }
            Expr::Instantiate { function, args } => self.instantiate(function, args)?,
            Expr::Call { callee, args } => self.call(callee, args)?,
            Expr::Record(fields) => {
                let mut cores = Vec::new();
                let mut types = Vec::new();
                for (name, value) in fields {
                    let (core, ty) = self.infer(value)?;
                    cores.push((name.clone(), core));
                    types.push((name.clone(), ty));
                }
                types.sort_by(|(a, _), (b, _)| a.cmp(b));
                (core::Expr::Record(cores), Ty::Record(types))
            }
            Expr::Tuple(members) => {
                let (cores, types) = self.infer_all(members)?;
                (core::Expr::Tuple(cores), Ty::Tuple(types))
            }
            Expr::List(members) => {
                let (cores, types) = self.infer_all(members)?;
                let member = self.env.union(types);
                (core::Expr::List(cores), Ty::List(Box::new(member)))
            }
            Expr::Field { target, name } => {
                let (core, ty) = self.infer(target)?;
                let field = self.field(&ty, name).ok_or_else(|| Error::NoField {
                    symbol: self.symbol.clone(),
                    ty: self.env.show(&ty),
                    name: name.clone(),
                })?;
                let core = core::Expr::Field {
                    target: Box::new(core),
                    name: name.clone(),
                };
                (core, field)
            }
            Expr::Let { value, body, .. } => {
                let (value, ty) = self.infer(value)?;
                self.locals.push(ty);
                let (body, ty) = self.infer(body)?;
                let core = core::Expr::Let {
                    value: Box::new(value),
                    body: Box::new(body),
                };
                (core, ty)
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => self.binary(*operator, left, right)?,
            Expr::Match { subject, arms } => self.matching(subject, arms)?,
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => {
                let (value, _) = self.infer(value)?;
                self.well_formed(ty)?;
                let target = self.env.normalize(ty);
                let (otherwise, alternative) = self.infer(otherwise)?;
                let core = core::Expr::Cast {
                    value: Box::new(value),
                    ty: self.lower(&target),
                    otherwise: Box::new(otherwise),
                };
                (core, self.env.union(vec![target, alternative]))
            }
        })
    }

    /// A generic function at type arguments that were written out.
    fn instantiate(
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

    fn call(
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
        let Ty::Function { params, ret } = self.env.unfold(&ty) else {
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

    /// A call to a generic function: its type arguments are whatever the
    /// arguments require.
    fn generic_call(
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
        let (core_args, found) = self.infer_all(args)?;
        let mut lower = vec![Vec::new(); signature.type_params.len()];
        for (param, found) in signature.params.iter().zip(&found) {
            self.collect(param, found, &mut lower);
        }
        let type_args = self.solve(function, signature, lower)?;
        for (param, found) in signature.params.iter().zip(&found) {
            let expected = self.env.normalize(&substitute(param, &type_args));
            if !self.env.subtype(found, &expected) {
                return Err(self.mismatch(&expected, found));
            }
        }
        let core = core::Expr::Call {
            callee: Box::new(self.reference(function, &type_args)),
            args: core_args,
        };
        let ret = self.env.normalize(&substitute(&signature.ret, &type_args));
        Ok((core, ret))
    }

    /// The header of `function`, if it has type parameters.
    fn generic(&self, function: &Symbol) -> Option<Signature> {
        self.env
            .functions
            .get(function)
            .filter(|signature| !signature.type_params.is_empty())
            .cloned()
    }

    fn reference(&mut self, function: &Symbol, type_args: &[Ty]) -> core::Expr {
        let type_args = type_args.iter().map(|arg| self.lower(arg)).collect();
        core::Expr::Reference(self.slot(function), type_args)
    }

    /// Records what `found` requires of the type parameters in `pattern`,
    /// a type from a generic header, for `found` to be a subtype of it.
    ///
    /// Only positions where a parameter must be at least some type are
    /// read. The result is verified by subtyping afterwards, so missing a
    /// requirement here costs an error message, never soundness.
    fn collect(&self, pattern: &Ty, found: &Ty, lower: &mut [Vec<Ty>]) {
        self.collect_within(pattern, found, lower, 0);
    }

    fn collect_within(&self, pattern: &Ty, found: &Ty, lower: &mut [Vec<Ty>], depth: usize) {
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
    fn solve(
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

    fn within_bounds(&self, signature: &Signature, args: &[Ty]) -> Result<(), Error> {
        for (arg, bound) in args.iter().zip(&signature.type_params) {
            let bound = self.env.normalize(&substitute(bound, args));
            if !self.env.subtype(arg, &bound) {
                return Err(self.mismatch(&bound, arg));
            }
        }
        Ok(())
    }

    /// Fails if a type applies a definition to the wrong number of
    /// arguments.
    fn well_formed(&self, ty: &Ty) -> Result<(), Error> {
        let mut result = Ok(());
        visit_types(ty, &mut |ty| {
            let Ty::Named(name, args) = ty else { return };
            let expected = match name {
                Reference::Definition(symbol) => self.env.types.get(symbol).map(|info| info.params),
                Reference::Local(_) | Reference::Parameter(_) => Some(0),
            };
            if let Some(expected) = expected
                && expected != args.len()
            {
                result = Err(Error::TypeArguments {
                    symbol: self.symbol.clone(),
                    expected,
                    found: args.len(),
                });
            }
        });
        result
    }

    /// The type of field `name` of a value of type `ty`, if every such
    /// value has one.
    fn field(&self, ty: &Ty, name: &str) -> Option<Ty> {
        match self.env.unfold(ty) {
            Ty::Never => Some(Ty::Never),
            Ty::Named(Reference::Parameter(position), _) => {
                self.field(self.env.bounds.get(position)?, name)
            }
            Ty::Record(fields) => field(&fields, name).cloned(),
            Ty::Union(members) => {
                let fields: Option<Vec<Ty>> = members.iter().map(|m| self.field(m, name)).collect();
                fields.map(|fields| self.env.union(fields))
            }
            _ => None,
        }
    }

    fn binary(
        &mut self,
        operator: Operator,
        left: &Expr<Reference>,
        right: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (core_left, left) = self.infer(left)?;
        let (core_right, right) = self.infer(right)?;
        let both = |ty: &Ty| self.env.subtype(&left, ty) && self.env.subtype(&right, ty);
        let choice = match operator {
            Operator::Equal => Some((Primitive::Equal, Ty::Bool)),
            _ if both(&Ty::Int) => Some(match operator {
                Operator::Add => (Primitive::IntAdd, Ty::Int),
                Operator::Subtract => (Primitive::IntSubtract, Ty::Int),
                Operator::Multiply => (Primitive::IntMultiply, Ty::Int),
                Operator::Less | Operator::Equal => (Primitive::IntLess, Ty::Bool),
            }),
            _ if both(&Ty::Float) => Some(match operator {
                Operator::Add => (Primitive::FloatAdd, Ty::Float),
                Operator::Subtract => (Primitive::FloatSubtract, Ty::Float),
                Operator::Multiply => (Primitive::FloatMultiply, Ty::Float),
                Operator::Less | Operator::Equal => (Primitive::FloatLess, Ty::Bool),
            }),
            _ => None,
        };
        let Some((operation, ty)) = choice else {
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                found: format!("{}, {}", self.env.show(&left), self.env.show(&right)),
            });
        };
        let core = core::Expr::Primitive {
            operation,
            args: vec![core_left, core_right],
        };
        Ok((core, ty))
    }

    fn matching(
        &mut self,
        subject: &Expr<Reference>,
        arms: &[Arm<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (subject, mut remaining) = self.infer(subject)?;
        for arm in arms {
            let mut result = Ok(());
            pattern_types(&arm.pattern, &mut |ty| {
                if result.is_ok() {
                    result = self.well_formed(ty);
                }
            });
            result?;
        }
        let mut core_arms = Vec::new();
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            // An arm sees only what earlier arms let through.
            let Some(matched) = self.narrow(&remaining, &arm.pattern) else {
                return Err(Error::UnreachableArm {
                    symbol: self.symbol.clone(),
                    arm: index,
                });
            };
            let pattern = self.bind(&matched, &arm.pattern);
            let (body, ty) = self.infer(&arm.body)?;
            core_arms.push((pattern, body));
            results.push(ty);
            remaining = self.remainder(&remaining, &arm.pattern);
        }
        if remaining != Ty::Never {
            return Err(Error::NotExhaustive {
                symbol: self.symbol.clone(),
                remaining: self.env.show(&remaining),
            });
        }
        let core = core::Expr::Match {
            subject: Box::new(subject),
            arms: core_arms,
        };
        Ok((core, self.env.union(results)))
    }

    /// The values of type `ty` that `pattern` matches, as a type; `None` if
    /// there are none.
    fn narrow(&self, ty: &Ty, pattern: &Pat) -> Option<Ty> {
        let ty = self.env.unfold(ty);
        if let Ty::Union(members) = &ty {
            let parts: Vec<Ty> = members
                .iter()
                .filter_map(|member| self.narrow(member, pattern))
                .collect();
            return (!parts.is_empty()).then(|| self.env.union(parts));
        }
        if ty == Ty::Never {
            return None;
        }
        match pattern {
            Pattern::Wildcard => Some(ty),
            Pattern::Bind(_, inner) => self.narrow(&ty, inner),
            Pattern::Literal(literal) => {
                let literal = Ty::Literal(literal.clone());
                self.env.subtype(&literal, &ty).then_some(literal)
            }
            Pattern::Is(tested) => {
                let tested = self.env.normalize(tested);
                if self.env.subtype(&ty, &tested) {
                    Some(ty)
                } else if self.env.disjoint(&ty, &tested) {
                    None
                } else {
                    // The value is described by both types. Either is a
                    // sound answer; the tested type is the one asked for.
                    Some(tested)
                }
            }
            Pattern::Record(patterns) => {
                let mut fields = match ty {
                    Ty::Record(fields) => fields,
                    Ty::Unknown => Vec::new(),
                    _ => return None,
                };
                for (name, pattern) in patterns {
                    // A field the type does not mention may still be there.
                    let have = field(&fields, name).cloned().unwrap_or(Ty::Unknown);
                    let narrowed = self.narrow(&have, pattern)?;
                    fields.retain(|(field, _)| field != name);
                    fields.push((name.clone(), narrowed));
                }
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Some(Ty::Record(fields))
            }
            Pattern::Tuple(patterns) => {
                let members: Vec<Ty> = match ty {
                    Ty::Tuple(members) if members.len() == patterns.len() => members,
                    // A list type describes tuples of every length.
                    Ty::List(member) => vec![*member; patterns.len()],
                    Ty::Unknown => vec![Ty::Unknown; patterns.len()],
                    _ => return None,
                };
                let narrowed: Option<Vec<Ty>> = members
                    .iter()
                    .zip(patterns)
                    .map(|(member, pattern)| self.narrow(member, pattern))
                    .collect();
                narrowed.map(Ty::Tuple)
            }
        }
    }

    /// Whether `pattern` matches every value of type `ty`.
    fn covers(&self, ty: &Ty, pattern: &Pat) -> bool {
        let ty = self.env.unfold(ty);
        if let Ty::Union(members) = &ty {
            return members.iter().all(|member| self.covers(member, pattern));
        }
        match pattern {
            Pattern::Wildcard => true,
            Pattern::Bind(_, inner) => self.covers(&ty, inner),
            Pattern::Literal(literal) => ty == Ty::Literal(literal.clone()),
            Pattern::Is(tested) => self.env.subtype(&ty, &self.env.normalize(tested)),
            Pattern::Record(patterns) => match &ty {
                Ty::Record(fields) => patterns.iter().all(|(name, pattern)| {
                    field(fields, name).is_some_and(|ty| self.covers(ty, pattern))
                }),
                _ => false,
            },
            Pattern::Tuple(patterns) => match &ty {
                Ty::Tuple(members) => {
                    members.len() == patterns.len()
                        && members.iter().zip(patterns).all(|(m, p)| self.covers(m, p))
                }
                _ => false,
            },
        }
    }

    /// The values of type `ty` that `pattern` may fail to match, as a type.
    /// It may describe more values than that, never fewer.
    fn remainder(&self, ty: &Ty, pattern: &Pat) -> Ty {
        let ty = self.env.unfold(ty);
        match &ty {
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.remainder(m, pattern)).collect();
                return self.env.union(parts);
            }
            Ty::Bool => {
                let parts = [false, true]
                    .map(|value| self.remainder(&Ty::Literal(Literal::Bool(value)), pattern));
                return self.env.union(parts.to_vec());
            }
            _ => {}
        }
        if self.covers(&ty, pattern) {
            return Ty::Never;
        }
        // When exactly one member of a record or tuple pattern can fail,
        // what is left is the same type with that member narrowed.
        match (pattern, &ty) {
            (Pattern::Bind(_, inner), _) => self.remainder(&ty, inner),
            (Pattern::Record(patterns), Ty::Record(fields)) => {
                let all_present = patterns
                    .iter()
                    .all(|(name, _)| field(fields, name).is_some());
                let mut open = patterns.iter().filter(|(name, pattern)| {
                    !field(fields, name).is_some_and(|ty| self.covers(ty, pattern))
                });
                let (Some((name, pattern)), None, true) = (open.next(), open.next(), all_present)
                else {
                    return ty;
                };
                let fields = fields
                    .iter()
                    .map(|(field, ty)| {
                        let ty = if field == name {
                            self.remainder(ty, pattern)
                        } else {
                            ty.clone()
                        };
                        (field.clone(), ty)
                    })
                    .collect();
                uninhabited_if_member_is(Ty::Record(fields))
            }
            (Pattern::Tuple(patterns), Ty::Tuple(members)) if patterns.len() == members.len() => {
                let mut open = (0..members.len())
                    .filter(|index| !self.covers(&members[*index], &patterns[*index]));
                let (Some(index), None) = (open.next(), open.next()) else {
                    return ty;
                };
                let mut members = members.clone();
                members[index] = self.remainder(&members[index], &patterns[index]);
                uninhabited_if_member_is(Ty::Tuple(members))
            }
            _ => ty,
        }
    }

    /// Gives every binder in `pattern` its type, in binding order, and
    /// returns the core pattern. `ty` is what the pattern matched.
    fn bind(&mut self, ty: &Ty, pattern: &Pat) -> core::Pattern {
        let ty = self.env.unfold(ty);
        match pattern {
            Pattern::Wildcard => core::Pattern::Wildcard,
            Pattern::Literal(literal) => core::Pattern::Literal(literal.clone()),
            Pattern::Is(tested) => {
                let tested = self.env.normalize(tested);
                core::Pattern::Is(self.lower(&tested))
            }
            Pattern::Bind(_, inner) => {
                self.locals.push(ty.clone());
                core::Pattern::Bind(Box::new(self.bind(&ty, inner)))
            }
            Pattern::Record(patterns) => core::Pattern::Record(
                patterns
                    .iter()
                    .map(|(name, pattern)| {
                        let member = self.field(&ty, name).unwrap_or(Ty::Unknown);
                        (name.clone(), self.bind(&member, pattern))
                    })
                    .collect(),
            ),
            Pattern::Tuple(patterns) => core::Pattern::Tuple(
                patterns
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        let member = self.member(&ty, index);
                        self.bind(&member, pattern)
                    })
                    .collect(),
            ),
        }
    }

    /// The type of member `index` of a tuple-shaped value of type `ty`.
    fn member(&self, ty: &Ty, index: usize) -> Ty {
        match ty {
            Ty::Tuple(members) => members.get(index).cloned().unwrap_or(Ty::Unknown),
            Ty::List(member) => (**member).clone(),
            Ty::Union(members) => {
                let parts = members.iter().map(|m| self.member(m, index)).collect();
                self.env.union(parts)
            }
            _ => Ty::Unknown,
        }
    }
}

/// A record or tuple with a member no value can have has no values itself.
fn uninhabited_if_member_is(ty: Ty) -> Ty {
    let empty = match &ty {
        Ty::Record(fields) => fields.iter().any(|(_, ty)| *ty == Ty::Never),
        Ty::Tuple(members) => members.contains(&Ty::Never),
        _ => false,
    };
    if empty { Ty::Never } else { ty }
}

/// The type of a function with this header, at these type arguments.
fn function_type(signature: &Signature, type_args: &[Ty]) -> Ty {
    Ty::Function {
        params: signature
            .params
            .iter()
            .map(|param| substitute(param, type_args))
            .collect(),
        ret: Box::new(substitute(&signature.ret, type_args)),
    }
}

/// Calls `visit` with every type a pattern tests against.
fn pattern_types(pattern: &Pat, visit: &mut impl FnMut(&Ty)) {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Record(fields) => fields.iter().for_each(|(_, p)| pattern_types(p, visit)),
        Pattern::Tuple(members) => members.iter().for_each(|p| pattern_types(p, visit)),
        Pattern::Is(ty) => visit(ty),
        Pattern::Bind(_, inner) => pattern_types(inner, visit),
    }
}
