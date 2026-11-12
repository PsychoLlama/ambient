//! Tasks, budgets, boundaries, and abilities: the nodes that decide what
//! runs where, and under which bindings.

use super::expr::{Body, Layer};
use super::{Invalid, shape};
use crate::core::{self, Expr};
use crate::types::{Ty, error};

impl Body<'_, '_> {
    /// What a closure that takes nothing, from its caller or as
    /// arguments, gives: the body of a task or a cleanup.
    fn thunk(&mut self, expr: &Expr) -> Result<Ty, Invalid> {
        let found = self.infer(expr)?;
        match self.verifier.env.unfold(&found) {
            Ty::Function { params, ret, row } if params.is_empty() && row.is_empty() => Ok(*ret),
            _ => {
                let expected = Ty::Function {
                    params: Vec::new(),
                    ret: Box::new(Ty::Unknown),
                    row: Vec::new(),
                };
                Err(self.verifier.mismatch(&expected, &found))
            }
        }
    }

    /// The type of a node that is not a plain computation.
    pub(super) fn effect(&mut self, expr: &Expr) -> Result<Ty, Invalid> {
        let verifier = self.verifier;
        let env = &verifier.env;
        Ok(match expr {
            Expr::Cleanup { cleanup, body } => {
                self.thunk(cleanup)?;
                self.infer(body)?
            }
            Expr::Budget {
                steps,
                allocation,
                body,
            } => {
                self.check(steps, &Ty::Int)?;
                self.check(allocation, &Ty::Int)?;
                let ty = self.infer(body)?;
                env.union(vec![ty, Ty::Tag])
            }
            Expr::Boundary(body) => {
                let ty = self.infer(body)?;
                env.union(vec![ty, env.normalize(&error())])
            }
            Expr::Group(body) => {
                self.groups += 1;
                let ty = self.infer(body);
                self.groups -= 1;
                ty?
            }
            Expr::Spawn(task) => {
                if self.groups == 0 {
                    return shape("a task is spawned outside every group");
                }
                self.thunk(task)?;
                Ty::Null
            }
            Expr::All(branches) => {
                let results: Result<Vec<Ty>, Invalid> =
                    branches.iter().map(|branch| self.thunk(branch)).collect();
                Ty::Tuple(results?)
            }
            Expr::First(branches) => {
                let results: Result<Vec<Ty>, Invalid> =
                    branches.iter().map(|branch| self.thunk(branch)).collect();
                env.union(results?)
            }
            Expr::AbilityId(ability) => {
                verifier.ability(*ability)?;
                Ty::String
            }
            Expr::Perform {
                ability,
                args,
                returns,
            } => {
                let (ability, info) = verifier.ability(*ability)?;
                if info.params.len() != args.len() {
                    return Err(Invalid::Arity);
                }
                if *returns != (info.ret != Ty::Never) {
                    return shape("a perform is wrong about whether its ability returns");
                }
                self.require(ability)?;
                for (arg, param) in args.iter().zip(&info.params) {
                    self.check(arg, param)?;
                }
                info.ret.clone()
            }
            Expr::Provide {
                handlers,
                only,
                body,
            } => self.provide(handlers, *only, body)?,
            _ => return shape("a node the core checker does not know"),
        })
    }

    /// A provide: its handlers are evaluated where it is written, and
    /// its body where they are bound.
    fn provide(
        &mut self,
        handlers: &[core::Handler],
        only: bool,
        body: &Expr,
    ) -> Result<Ty, Invalid> {
        let verifier = self.verifier;
        let env = &verifier.env;
        let mut bound = Vec::new();
        // What the provide may give besides its body: the result of each
        // handler that aborts to it.
        let mut results = Vec::new();
        for handler in handlers {
            let (ability, info) = verifier.ability(handler.ability)?;
            let found = self.infer(&handler.handler)?;
            let aborts = info.ret == Ty::Never;
            if handler.abort != aborts.then_some(info.params.len()) {
                return shape("a handler is wrong about whether its ability returns");
            }
            let expected = Ty::Function {
                params: info.params.clone(),
                ret: Box::new(if aborts {
                    Ty::Unknown
                } else {
                    info.ret.clone()
                }),
                row: Vec::new(),
            };
            if aborts {
                // It runs at the provide, so it may take what is bound
                // there, and what it gives the provide gives.
                let Ty::Function { params, ret, row } = env.unfold(&found) else {
                    return Err(verifier.mismatch(&expected, &found));
                };
                let fits = params.len() == info.params.len()
                    && info
                        .params
                        .iter()
                        .zip(&params)
                        .all(|(given, taken)| env.subtype(given, taken));
                if !fits {
                    return Err(verifier.mismatch(&expected, &found));
                }
                self.require_all(&row)?;
                results.push(*ret);
            } else if !env.subtype(&found, &expected) {
                return Err(verifier.mismatch(&expected, &found));
            }
            if bound.contains(ability) {
                return shape("a provide binds one ability twice");
            }
            bound.push(ability.clone());
        }
        self.layers.push(Layer {
            abilities: bound,
            only,
        });
        let ty = self.infer(body);
        self.layers.pop();
        results.push(ty?);
        Ok(env.union(results))
    }
}
