//! The type checker: a resolved definition and an [`Env`] yield an open core
//! definition. See `spec/types.md` and `spec/core.md`.
//!
//! Calls to generic functions are in [`generics`]; match is in
//! [`patterns`].
//!
//! Checking is a pure function of its arguments. Everything it knows about
//! other definitions is in the `Env`, which is what lets the engine key its
//! result by content.

mod generics;
mod patterns;

use std::collections::BTreeMap;

use crate::core::{self, Open, Primitive, Slot, Tree};
use crate::error::Error;
use crate::surface::{Expr, Function, Operator, Reference, TypeDefinition};
use crate::symbol::Symbol;
use crate::types::{Env, Ty, field};
use generics::function_type;

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
    for param in header.params {
        checker.bind_local(param);
    }
    let body = checker.check(&function.body, &header.ret)?;
    let frame = checker.finish();
    Ok(Open {
        tree: Tree::Function(core::Function {
            type_params,
            captures: Vec::new(),
            params,
            ret,
            body,
        }),
        slots: frame.slots,
        lambdas: frame.lambdas,
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
        slots: checker.finish().slots,
        lambdas: Vec::new(),
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
    Ok((encoding, checker.finish().slots))
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

/// What is being built for one definition: the named one, or a lambda
/// inside it. Each has its own slots and its own numbering of locals.
#[derive(Default)]
struct Frame {
    /// Where this definition sits among the lambdas of the named one.
    path: Vec<usize>,
    slots: Vec<Symbol>,
    /// The core number of each local bound here, by resolved number.
    locals: BTreeMap<usize, usize>,
    /// Locals of enclosing definitions that this one uses, by resolved
    /// number, in order of first use.
    captures: Vec<usize>,
    lambdas: Vec<Open>,
}

struct Checker<'a> {
    symbol: &'a Symbol,
    env: &'a Env,
    /// The definition being checked, then each lambda being checked inside
    /// it, innermost last.
    frames: Vec<Frame>,
    /// The type of every local bound so far, by resolved number.
    types: BTreeMap<usize, Ty>,
    /// The resolved number the next binder has. Resolution numbers binders
    /// in the order they are written; this follows it explicitly so that
    /// expressions can be checked in another order.
    next: usize,
}

impl<'a> Checker<'a> {
    fn new(symbol: &'a Symbol, env: &'a Env) -> Self {
        Self {
            symbol,
            env,
            frames: vec![Frame::default()],
            types: BTreeMap::new(),
            next: 0,
        }
    }

    fn frame(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            self.frames.push(Frame::default());
        }
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    /// The outermost frame, once checking is over.
    fn finish(mut self) -> Frame {
        self.frames.drain(..).next().unwrap_or_default()
    }

    fn slot(&mut self, symbol: &Symbol) -> Slot {
        let slots = &mut self.frame().slots;
        let index = slots
            .iter()
            .position(|slot| slot == symbol)
            .unwrap_or_else(|| {
                slots.push(symbol.clone());
                slots.len() - 1
            });
        Slot(index)
    }

    /// Binds the next local, in the definition being checked.
    fn bind_local(&mut self, ty: Ty) {
        let number = self.next;
        self.next += 1;
        self.types.insert(number, ty);
        let frame = self.frame();
        let index = frame.locals.len();
        frame.locals.insert(number, index);
    }

    /// How the frame at `depth` reaches the local with this resolved
    /// number: directly, or by capturing it from the frame outside.
    fn reach(&mut self, depth: usize, number: usize) -> core::Expr {
        let frame = &mut self.frames[depth];
        if let Some(index) = frame.locals.get(&number) {
            return core::Expr::Local(*index);
        }
        let position = frame
            .captures
            .iter()
            .position(|captured| *captured == number)
            .unwrap_or_else(|| {
                frame.captures.push(number);
                frame.captures.len() - 1
            });
        core::Expr::Captured(position)
    }

    /// A lambda: its own definition, checked in a frame of its own.
    ///
    /// `hint` is the parameter types expected of it, if anything expects
    /// any. The result type is always the body's.
    fn lambda(
        &mut self,
        params: &[(String, Option<Ty>)],
        body: &Expr<Reference>,
        hint: Option<&[Ty]>,
    ) -> Result<(core::Expr, Ty), Error> {
        let mut types = Vec::new();
        for (position, (name, ty)) in params.iter().enumerate() {
            types.push(match (ty, hint.and_then(|hint| hint.get(position))) {
                (Some(ty), _) => {
                    self.well_formed(ty)?;
                    self.env.normalize(ty)
                }
                (None, Some(hint)) => hint.clone(),
                (None, None) => {
                    return Err(Error::UntypedParameter {
                        symbol: self.symbol.clone(),
                        name: name.clone(),
                    });
                }
            });
        }

        let mut path = self.frame().path.clone();
        path.push(self.frame().lambdas.len());
        self.frames.push(Frame {
            path,
            ..Frame::default()
        });
        for ty in &types {
            self.bind_local(ty.clone());
        }
        let (body, ret) = self.infer(body)?;

        // The lambda's own tree is lowered while its frame is current, so
        // the slots land in its table.
        let type_params: Vec<_> = self.env.bounds.iter().map(|ty| self.lower(ty)).collect();
        let core_params = types.iter().map(|ty| self.lower(ty)).collect();
        let core_ret = self.lower(&ret);
        let outer = self.frame().captures.clone();
        let captures = outer
            .iter()
            .map(|number| {
                let ty = self.types.get(number).cloned().unwrap_or(Ty::Never);
                self.lower(&ty)
            })
            .collect();
        let frame = self.frames.pop().unwrap_or_default();
        let symbol = self.symbol.lambda(&frame.path);
        let definition = Open {
            tree: Tree::Function(core::Function {
                type_params,
                captures,
                params: core_params,
                ret: core_ret,
                body,
            }),
            slots: frame.slots,
            lambdas: frame.lambdas,
        };
        self.frame().lambdas.push(definition);

        // The encloser hands over each captured value, capturing it in turn
        // if it is not its own.
        let depth = self.frames.len() - 1;
        let captures = outer
            .iter()
            .map(|number| self.reach(depth, *number))
            .collect();
        // A lambda has its encloser's type parameters, at themselves.
        let type_args = (0..self.env.bounds.len())
            .map(core::Type::Parameter)
            .collect();
        let core = core::Expr::Closure {
            slot: self.slot(&symbol),
            type_args,
            captures,
        };
        let ty = Ty::Function {
            params: types,
            ret: Box::new(ret),
        };
        Ok((core, ty))
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
        let (core, found) = match expr {
            // A lambda takes the types its parameters lack from the
            // function type expected of it.
            Expr::Lambda { params, body } => match self.env.unfold(expected) {
                Ty::Function { params: hint, .. } => self.lambda(params, body, Some(&hint))?,
                _ => self.lambda(params, body, None)?,
            },
            _ => self.infer(expr)?,
        };
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
                let ty = self.types.get(number).cloned().unwrap_or(Ty::Never);
                let depth = self.frames.len().saturating_sub(1);
                (self.reach(depth, *number), ty)
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
                self.bind_local(ty);
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
            Expr::Lambda { params, body } => self.lambda(params, body, None)?,
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
}
