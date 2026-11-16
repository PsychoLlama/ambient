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
mod blame;
mod calls;
mod code;
mod comptime;
mod definitions;
mod formed;
mod generics;
mod header;
mod lambda;
mod methods;
mod observe;
mod operators;
mod patterns;
mod records;
mod reflect;
mod reify;
mod sends;
mod tasks;
mod world;

use std::collections::{BTreeMap, HashMap};

use crate::core::{self, Open, Slot, Tree};
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Expr, Function, Reference};
use crate::symbol::{PackageId, Symbol};
use crate::types::{Env, Ty};
use crate::value::Value;
use abilities::Layer;
pub use blame::{Noted, Origins};
pub use definitions::{defined, definitions};
pub(crate) use generics::{function_type, generic_type};
use header::header;
pub use header::{check_ability, check_key, check_type, encode_header};
pub use reify::{closed, function, module, tree};
pub use world::{check_extern, check_target};

/// Checks a function against its own header.
///
/// # Errors
///
/// Returns the first type error in the function. Either way, `noted`
/// is what checking found out about where things are.
pub fn check_function(
    symbol: &Symbol,
    function: &Function<Reference>,
    mut env: Env,
    evaluator: &dyn Evaluate,
    root: bool,
    noted: &mut Noted,
) -> Result<Open, Error> {
    let header = header(symbol, function, &mut env)?;
    let mut checker = Checker::new(symbol, &env);
    checker.place(&function.body);
    checker.evaluator = Some(evaluator);
    checker.root = root;
    let type_params = env.bounds.iter().map(|ty| checker.lower(ty)).collect();
    let params = header.params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&header.ret);
    let row = checker.row(&header.row);
    checker.frame().layers.push(Layer {
        abilities: header.row,
        only: false,
        seen: None,
    });
    for param in header.params {
        checker.bind_local(param);
    }
    let body = checker.check(&function.body, &header.ret);
    noted.at = checker.blamed_in(&function.body);
    noted.reached = std::mem::take(&mut checker.reached);
    noted.given = checker.given.take();
    let mut body = body?;
    checker.origins.insert(Vec::new(), body.origins(0));
    noted.origins = std::mem::take(&mut checker.origins);
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
    /// How many layers there were where each group checking is inside of
    /// began, innermost last.
    groups: Vec<usize>,
    /// Abilities this definition uses that only an enclosing one binds.
    bindings: Vec<Symbol>,
    lambdas: Vec<Open>,
    /// How many type parameters this definition has of its own, after
    /// those of what it is written in: a generic lambda's.
    generic: usize,
}

/// Runs functions for comptime calls.
///
/// An answer must depend only on the function's hash and the arguments,
/// so that a checked definition is still a function of what it was keyed
/// by.
pub trait Evaluate {
    /// Calls `function` with `args`, for a call written in `from`.
    ///
    /// # Errors
    ///
    /// Fails if the function does not compile, or faults.
    fn evaluate(&self, from: &PackageId, function: &Symbol, args: &[Value])
    -> Result<Value, Error>;

    /// Whether the comptime calls of `package` may take `ability` from
    /// the build target.
    fn granted(&self, package: &PackageId, ability: &Symbol) -> bool;

    /// Whether code written in `package` could name `symbol`: it is of
    /// that package, or public in a module its own package lets others
    /// reach.
    fn nameable(&self, package: &PackageId, symbol: &Symbol) -> bool;
}

struct Checker<'a> {
    symbol: &'a Symbol,
    env: &'a Env,
    /// What runs comptime calls. A header has none to run.
    evaluator: Option<&'a dyn Evaluate>,
    /// Whether this is a program's root, which alone may name a native
    /// handler.
    root: bool,
    /// Whether whatever is being checked has every ability bound for it:
    /// true inside a comptime call, which the build target provides for.
    providing: bool,
    /// How many locals spliced code has bound so far.
    spliced: usize,
    /// The definitions spliced code named.
    reached: Vec<Symbol>,
    /// The definition being checked, then each lambda being checked inside
    /// it, innermost last.
    frames: Vec<Frame>,
    /// The type of every local bound so far, by resolved number.
    types: BTreeMap<usize, Ty>,
    /// The resolved number the next binder has. Resolution numbers binders
    /// in the order they are written; this follows it explicitly so that
    /// expressions can be checked in another order.
    next: usize,
    /// How many expressions the one being checked is inside.
    nesting: usize,
    /// The innermost expression the first error was at.
    blamed: Option<*const Expr<Reference>>,
    /// See [`Noted::given`].
    given: Option<String>,
    /// The number of each node of the body being checked.
    places: HashMap<*const Expr<Reference>, u32>,
    /// What each definition finished so far was checked from: see
    /// [`Noted::origins`].
    origins: Origins,
}

impl<'a> Checker<'a> {
    fn new(symbol: &'a Symbol, env: &'a Env) -> Self {
        Self {
            symbol,
            env,
            evaluator: None,
            root: false,
            providing: false,
            spliced: 0,
            reached: Vec::new(),
            frames: vec![Frame::default()],
            types: BTreeMap::new(),
            next: 0,
            nesting: 0,
            blamed: None,
            given: None,
            places: HashMap::new(),
            origins: BTreeMap::new(),
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
        let env = self.env;
        env.lower(ty, &mut |symbol| self.slot(symbol))
    }

    fn mismatch(&self, expected: &Ty, found: &Ty) -> Error {
        Error::Mismatch {
            symbol: self.symbol.clone(),
            expected: self.env.show(expected),
            found: self.env.show(found),
        }
    }

    /// The core form of `expr` and its type, where `expected` may supply
    /// what `expr` leaves out. Nothing here says the type fits.
    fn infer_as(
        &mut self,
        expr: &Expr<Reference>,
        expected: &Ty,
    ) -> Result<(core::Expr, Ty), Error> {
        let expected = self.env.unfold(expected);
        // What a lambda that states no row takes from its caller: the
        // row of the function type expected of it.
        let asked = match &expected {
            Ty::Function { row, .. } => row.clone(),
            _ => Vec::new(),
        };
        let hint = match (expected, expr) {
            (Ty::Function { params, .. }, _) => Some(params),
            // A generic lambda takes them from a generic function type
            // with as many type parameters, each standing for its own.
            (Ty::Generic { bounds, function }, Expr::Lambda { type_params, .. })
                if bounds.len() == type_params.len() =>
            {
                self.hinted(&bounds, &function)
            }
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
                let args = self.solve(Some(function), &signature, 0, lower)?;
                let found = self.env.normalize(&function_type(&signature, &args));
                Ok((self.reference(function, &args), found))
            }
            // A lambda takes the types its parameters lack from the
            // function type expected of it.
            (Expr::Lambda { .. }, hint) => self.lambda(expr, hint.as_deref(), &asked),
            // Any other value of a generic function type takes its
            // type arguments from the function type expected of it too.
            (_, Some(params)) => {
                let (core, found) = self.infer(expr)?;
                if matches!(self.env.unfold(&found), Ty::Generic { .. }) {
                    return self.coerced((core, found), &params);
                }
                Ok((core, found))
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
        let inferred = self.entered(|checker| checker.infer_node(expr));
        let (core, ty) = self.blame(expr, inferred)?;
        Ok((self.marked(expr, core), ty))
    }

    /// The value a name gives if what it names is a tag: a type that is
    /// the tag told apart by where that very definition is declared.
    fn tag(&self, symbol: &Symbol) -> Option<Literal> {
        match &self.env.types.get(symbol)?.ty {
            Ty::Literal(tag @ Literal::Tag { tag: identity, .. })
                if *identity == symbol.identity() =>
            {
                Some(tag.clone())
            }
            _ => None,
        }
    }

    fn infer_node(&mut self, expr: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        Ok(match expr {
            Expr::Null => (core::Expr::Null, Ty::Null),
            Expr::Trace => self.capture(),
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
            // A generic function named where nothing says what types
            // it is used at is a value of a generic function type.
            Expr::Name(Reference::Definition(symbol)) => {
                if let Some(tag) = self.tag(symbol) {
                    return Ok((core::Expr::Literal(tag.clone()), Ty::Literal(tag)));
                }
                let ty = match self.env.functions.get(symbol) {
                    Some(signature) if self.root || !signature.native => {
                        self.env.normalize(&generic_type(signature))
                    }
                    _ => {
                        return Err(Error::NotValue {
                            symbol: self.symbol.clone(),
                            name: symbol.clone(),
                        });
                    }
                };
                (self.reference(symbol, &[]), ty)
            }
            Expr::Instantiate { function, args } => self.instantiate(function, args)?,
            Expr::Call { callee, args } => self.call(callee, args)?,
            Expr::Send {
                target,
                method,
                args,
            } => self.send(target, method, args)?,
            Expr::Record(fields) => self.record(fields)?,
            // Only a record's field is a method.
            Expr::Method(_) => return Err(self.not_method(&Ty::Unknown)),
            Expr::Merge { left, right } => self.merge(left, right)?,
            Expr::Tuple(members) => {
                let (cores, types) = self.infer_all(members)?;
                (core::Expr::Tuple(cores), Ty::Tuple(types))
            }
            Expr::List(members) => {
                let (cores, types) = self.infer_all(members)?;
                let member = self.env.union(types);
                (core::Expr::List(cores), Ty::List(Box::new(member)))
            }
            Expr::Field { target, key } => self.read(target, key)?,
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
            Expr::Operation { operator, operands } => self.operation(*operator, operands)?,
            Expr::Match { subject, arms } => self.matching(subject, arms, None)?,
            Expr::Lambda { .. } => self.lambda(expr, None, &[])?,
            Expr::Perform { ability, args } => self.perform(ability, args)?,
            Expr::Provide { bundle, only, body } => self.provide(bundle, *only, body)?,
            Expr::Observe { observer, body } => self.observe(observer, body)?,
            Expr::Using { value, body, .. } => self.using(value, body, None)?,
            // The reader takes its marks out, so one here is a tree that
            // was never built.
            Expr::At(..) => return Err(Error::UnknownSymbol(self.symbol.clone())),
            Expr::Group(body) => self.group(body)?,
            Expr::Budget {
                steps,
                allocation,
                body,
            } => self.budget(steps, allocation, body)?,
            Expr::Boundary(body) => self.boundary(body)?,
            Expr::Spawn(task) => self.spawn(task)?,
            Expr::All(branches) => self.join(branches, false)?,
            Expr::First(branches) => self.join(branches, true)?,
            Expr::Cell {
                operation,
                ty,
                args,
            } => self.cell(*operation, ty.as_ref(), args)?,
            Expr::AbilityId(ability) => match ability {
                Reference::Definition(ability) => {
                    (core::Expr::AbilityId(self.slot(ability)), Ty::String)
                }
                // Resolution only ever names an ability here.
                _ => (core::Expr::Null, Ty::Never),
            },
            Expr::Comptime(call) => self.comptime(call)?,
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => self.cast(value, ty, otherwise)?,
        })
    }
}
