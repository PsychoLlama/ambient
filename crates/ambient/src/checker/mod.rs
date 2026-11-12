//! The type checker: a resolved definition and an [`Env`] yield an open core
//! definition. See `spec/types.md` and `spec/core.md`.
//!
//! Calls to generic functions are in [`generics`]; match is in
//! [`patterns`].
//!
//! Checking is a pure function of its arguments. Everything it knows about
//! other definitions is in the `Env`, which is what lets the engine key its
//! result by content.

mod abilities;
mod comptime;
mod generics;
mod header;
mod lambda;
mod operators;
mod patterns;

use std::collections::BTreeMap;

use crate::core::{self, Open, Slot, Tree};
use crate::error::Error;
use crate::surface::{Expr, Function, Reference};
use crate::symbol::Symbol;
use crate::types::{Env, Ty, abilities};
use crate::value::Value;
use abilities::Layer;
use generics::function_type;
pub use header::{check_ability, check_type, encode_header};
use header::{header, visit_types};

/// Checks a function against its own header.
///
/// # Errors
///
/// Returns the first type error in the function.
pub fn check_function(
    symbol: &Symbol,
    function: &Function<Reference>,
    mut env: Env,
    evaluator: &dyn Evaluate,
) -> Result<Open, Error> {
    let header = header(symbol, function, &mut env)?;
    let mut checker = Checker::new(symbol, &env);
    checker.evaluator = Some(evaluator);
    let type_params = env.bounds.iter().map(|ty| checker.lower(ty)).collect();
    let params = header.params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&header.ret);
    let row = checker.row(&header.row);
    checker.frame().layers.push(Layer {
        abilities: header.row,
        only: false,
    });
    for param in header.params {
        checker.bind_local(param);
    }
    let body = checker.check(&function.body, &header.ret)?;
    let frame = checker.finish();
    Ok(Open {
        tree: Tree::Function(core::Function {
            type_params,
            captures: Vec::new(),
            bindings: Vec::new(),
            params,
            ret,
            row,
            body,
        }),
        slots: frame.slots,
        lambdas: frame.lambdas,
    })
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
    /// What binds abilities here, innermost last: the definition's row,
    /// then each provide checking is inside of.
    layers: Vec<Layer>,
    /// Abilities this definition uses that only an enclosing one binds.
    bindings: Vec<Symbol>,
    lambdas: Vec<Open>,
}

/// Runs functions for comptime calls.
///
/// An answer must depend only on the function's hash and the arguments,
/// so that a checked definition is still a function of what it was keyed
/// by.
pub trait Evaluate {
    /// Calls `function` with `args`.
    ///
    /// # Errors
    ///
    /// Fails if the function does not compile, or faults.
    fn evaluate(&self, function: &Symbol, args: &[Value]) -> Result<Value, Error>;
}

struct Checker<'a> {
    symbol: &'a Symbol,
    env: &'a Env,
    /// What runs comptime calls. A header has none to run.
    evaluator: Option<&'a dyn Evaluate>,
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
            evaluator: None,
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
            Ty::Function { params, ret, row } => core::Type::Function {
                params: params.iter().map(|p| self.lower(p)).collect(),
                ret: Box::new(self.lower(ret)),
                row: self.row(&abilities(row)),
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
        let (core, found) = self.infer_as(expr, expected)?;
        let (core, found) = self.bind_row(core, found, expected)?;
        if self.env.subtype(&found, expected) {
            Ok(core)
        } else {
            Err(self.mismatch(expected, &found))
        }
    }

    /// The core form of `expr` and its type, where `expected` may supply
    /// what `expr` leaves out. Nothing here says the type fits.
    fn infer_as(
        &mut self,
        expr: &Expr<Reference>,
        expected: &Ty,
    ) -> Result<(core::Expr, Ty), Error> {
        let hint = match self.env.unfold(expected) {
            Ty::Function { params, .. } => Some(params),
            _ => None,
        };
        match (expr, hint) {
            // A generic function used as a value takes its type arguments
            // from the function type expected of it.
            (Expr::Name(Reference::Definition(function)), Some(params))
                if self
                    .generic(function)
                    .is_some_and(|signature| signature.params.len() == params.len()) =>
            {
                let Some(signature) = self.generic(function) else {
                    return self.infer(expr);
                };
                let mut lower = vec![Vec::new(); signature.type_params.len()];
                for (param, wanted) in signature.params.iter().zip(&params) {
                    self.collect(param, wanted, &mut lower);
                }
                let args = self.solve(function, &signature, lower)?;
                let found = self.env.normalize(&function_type(&signature, &args));
                Ok((self.reference(function, &args), found))
            }
            // A lambda takes the types its parameters lack from the
            // function type expected of it.
            (Expr::Lambda { params, row, body }, hint) => {
                self.lambda(params, row, body, hint.as_deref())
            }
            _ => self.infer(expr),
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
            Expr::Lambda { params, row, body } => self.lambda(params, row, body, None)?,
            Expr::Perform { ability, args } => self.perform(ability, args)?,
            Expr::Provide {
                handlers,
                only,
                body,
            } => self.provide(handlers, *only, body)?,
            Expr::Comptime(call) => self.comptime(call)?,
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => self.cast(value, ty, otherwise)?,
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
}
