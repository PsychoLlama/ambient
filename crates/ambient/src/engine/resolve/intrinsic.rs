//! Calls of intrinsics: the names the compiler knows, each of which is
//! a node of a tree once it is called. See "Intrinsics" in
//! `docs/spec/definitions.md`.
//!
//! The node has fewer nodes inside it than the call that was written:
//! the name is gone, and so is the function a body was written as. So
//! each node that is left out is noted, and those a cast adds to hold
//! what it was given, which is how a node of the resolved tree is still
//! found in the text.

use ambient_runtime::FAULT_PACKAGE;

use super::Resolver;
use crate::error::Error;
use crate::surface::intrinsic::{Intrinsic, MODULE};
use crate::surface::{CellOp, Expr, Path, Reference, Type};
use crate::symbol::{ModuleId, Namespace, Symbol};

/// The intrinsic a symbol is, if it is one.
pub(super) fn intrinsic_of(symbol: &Symbol) -> Option<Intrinsic> {
    let declared = symbol.module == ModuleId::new(FAULT_PACKAGE, MODULE);
    let named = declared && symbol.namespace == Namespace::Value && symbol.lambda.is_empty();
    named.then(|| Intrinsic::named(&symbol.name)).flatten()
}

/// The number of each node of `body` that is one of `nodes`, as
/// [`Expr::children`] walks them, in order.
pub(super) fn numbered<N>(body: &Expr<N>, nodes: &[*const Expr<N>]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut passed = 0;
    let mut pending = vec![body];
    while let Some(expr) = pending.pop() {
        if nodes.contains(&std::ptr::from_ref(expr)) {
            found.push(passed);
        }
        passed += 1;
        let from = pending.len();
        expr.children(&mut |child| pending.push(child));
        // The first child is to be numbered first.
        pending[from..].reverse();
    }
    found
}

/// Each cast of `pairs` by its number in `body`, with the number of the
/// binding whose value it is paired with: the node before that value.
pub(super) fn stood<N>(
    body: &Expr<N>,
    pairs: &[(*const Expr<N>, *const Expr<N>)],
) -> Vec<(u32, u32)> {
    let number = |node: *const Expr<N>| numbered(body, &[node]).first().copied();
    let pair = |(cast, value): &(_, _)| Some((number(*cast)?, number(*value)?.checked_sub(1)?));
    pairs.iter().filter_map(pair).collect()
}

/// A call of an intrinsic, as it was written.
struct Written<'e> {
    intrinsic: Intrinsic,
    path: &'e Path,
    types: &'e [Type],
    args: &'e [Expr],
}

impl Resolver<'_> {
    /// Fails if `path` names an intrinsic: one is called, and is no
    /// value to hand on.
    pub(super) fn value(&self, path: &Path, named: Reference) -> Result<Reference, Error> {
        match &named {
            Reference::Definition(symbol) if intrinsic_of(symbol).is_some() => {
                Err(self.misused(path, "is called, and is not a value"))
            }
            _ => Ok(named),
        }
    }

    fn misused(&self, path: &Path, reason: &str) -> Error {
        Error::Intrinsic {
            symbol: self.symbol.clone(),
            path: path.clone(),
            reason: reason.to_owned(),
        }
    }

    /// The node a call is, if its callee names an intrinsic.
    pub(super) fn intrinsic(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Option<Result<Expr<Reference>, Error>> {
        let (path, types) = match callee {
            Expr::Name(path) => (path, &[][..]),
            Expr::Instantiate { function, args } => (function, args.as_slice()),
            _ => return None,
        };
        let Ok(Reference::Definition(symbol)) = self.name(path) else {
            return None;
        };
        let intrinsic = intrinsic_of(&symbol)?;
        self.dropped.push(std::ptr::from_ref(callee));
        let written = Written {
            intrinsic,
            path,
            types,
            args,
        };
        Some(self.node_of(&written))
    }

    fn node_of(&mut self, written: &Written) -> Result<Expr<Reference>, Error> {
        let Written { path, args, .. } = *written;
        let typed = matches!(
            written.intrinsic,
            Intrinsic::Cell(CellOp::New) | Intrinsic::Cast
        );
        if !typed && !written.types.is_empty() {
            return Err(self.misused(path, "takes no type"));
        }
        Ok(match written.intrinsic {
            Intrinsic::Operation(operator) => Expr::Operation {
                operator,
                operands: self.exprs(args)?,
            },
            Intrinsic::Cell(operation) => Expr::Cell {
                operation,
                ty: match written.types {
                    [] => None,
                    [ty] => Some(self.ty(ty)?),
                    _ => return Err(self.misused(path, "takes one type")),
                },
                args: self.exprs(args)?,
            },
            Intrinsic::AbilityId => {
                let [named @ Expr::Name(ability)] = args else {
                    return Err(self.misused(path, "takes the name of an ability"));
                };
                self.dropped.push(std::ptr::from_ref(named));
                Expr::AbilityId(self.ability(ability)?)
            }
            Intrinsic::Cast => {
                let ([ty], [value, fallback]) = (written.types, args) else {
                    return Err(self.misused(path, "is written `cast::<T>(value, fallback)`"));
                };
                self.cast(value, ty, fallback)?
            }
            Intrinsic::Contain => Expr::Boundary(Box::new(self.only_body(path, args)?)),
            Intrinsic::Group => Expr::Group(Box::new(self.only_body(path, args)?)),
            Intrinsic::Spawn => Expr::Spawn(Box::new(self.only_body(path, args)?)),
            Intrinsic::Observe => {
                let [observer, body] = args else {
                    return Err(self.misused(path, "takes an observer and a body"));
                };
                let observer = Box::new(self.expr(observer)?);
                // The binder a recording holds its observer in.
                self.next += 1;
                let body = Box::new(self.body(path, body)?);
                Expr::Observe { observer, body }
            }
            Intrinsic::All => Expr::All(self.bodies(path, args)?),
            Intrinsic::First => Expr::First(self.bodies(path, args)?),
        })
    }

    /// A cast as it is written: a call, so what it falls back to is
    /// evaluated before it, whether the value passes or not. A cast in
    /// a tree takes that as a branch, so one that could do anything is
    /// held first, after the value.
    fn cast(&mut self, value: &Expr, ty: &Type, fallback: &Expr) -> Result<Expr<Reference>, Error> {
        let ty = self.ty(ty)?;
        let value = Box::new(self.expr(value)?);
        if fallback.is_settled() {
            let otherwise = Box::new(self.expr(fallback)?);
            return Ok(Expr::Cast {
                value,
                ty,
                otherwise,
            });
        }
        let local = |number: usize| Box::new(Expr::Name(Reference::Local(number)));
        let held = self.next;
        self.next += 1;
        let fallback = Box::new(self.expr(fallback)?);
        let other = self.next;
        self.next += 1;
        let cast = Box::new(Expr::Cast {
            value: local(held),
            ty,
            otherwise: local(other),
        });
        let inner = Box::new(Expr::Let {
            name: "__fallback".to_owned(),
            value: fallback,
            body: cast,
        });
        // The outer binding is where the call was, and nothing wrote
        // the rest. An error in the cast is one in the call: the node
        // before the value, which the binding holds first.
        if let Expr::Let { body: cast, .. } = &*inner {
            self.added.push(std::ptr::from_ref(&*inner));
            self.added.push(std::ptr::from_ref(&**cast));
            cast.children(&mut |name| self.added.push(std::ptr::from_ref(name)));
            self.stood
                .push((std::ptr::from_ref(&**cast), std::ptr::from_ref(&*value)));
        }
        Ok(Expr::Let {
            name: "__cast".to_owned(),
            value,
            body: inner,
        })
    }

    fn only_body(&mut self, path: &Path, args: &[Expr]) -> Result<Expr<Reference>, Error> {
        match args {
            [body] => self.body(path, body),
            _ => Err(self.misused(path, "takes one body")),
        }
    }

    fn bodies(&mut self, path: &Path, args: &[Expr]) -> Result<Vec<Expr<Reference>>, Error> {
        args.iter().map(|body| self.body(path, body)).collect()
    }

    /// A body an intrinsic takes: a function of no parameters, written
    /// in place, which is run where the call is. What is resolved is
    /// what the function gives.
    fn body(&mut self, path: &Path, written: &Expr) -> Result<Expr<Reference>, Error> {
        let plain = |row: &[Path], returns: &Option<Type>| row.is_empty() && returns.is_none();
        match written {
            Expr::Lambda {
                type_params,
                params,
                row,
                returns,
                body,
            } if type_params.is_empty() && params.is_empty() && plain(row, returns) => {
                self.dropped.push(std::ptr::from_ref(written));
                self.expr(body)
            }
            _ => {
                // Most often what was written is the name of a function,
                // so say what to write around it.
                let reason = "takes a body written in place, `fn() => ...`: \
                              a function held in a value is called inside one";
                Err(self.misused(path, reason))
            }
        }
    }
}
