//! Tasks, boundaries, and abilities: the nodes that decide what
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
                    params: Vec::new().into(),
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
            Expr::Boundary(body) => {
                let ty = self.infer(body)?;
                env.union(vec![ty, env.normalize(&error())])
            }
            Expr::Group(body) => {
                self.groups.push(self.layers.len());
                let ty = self.infer(body);
                self.groups.pop();
                ty?
            }
            Expr::Spawn(task) => {
                let Some(group) = self.groups.last().copied() else {
                    return shape("a task is spawned outside every group");
                };
                // The task may outlast anything bound inside its group.
                if let Expr::Closure { bindings, .. } = &**task {
                    let inside = &self.layers[group..];
                    for ability in verifier.row(bindings)? {
                        if inside
                            .iter()
                            .any(|layer| layer.abilities.contains(&ability))
                        {
                            return shape("a task takes what is bound inside its group");
                        }
                    }
                }
                self.thunk(task)?;
                Ty::Null
            }
            Expr::All(branches) => {
                let results: Result<Vec<Ty>, Invalid> =
                    branches.iter().map(|branch| self.thunk(branch)).collect();
                Ty::Tuple(results?.into())
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
                types,
            } => {
                let (ability, info) = verifier.ability(*ability)?;
                if info.params.len() != args.len() {
                    return Err(Invalid::Arity);
                }
                if *returns != (info.ret != Ty::Never) {
                    return shape("a perform is wrong about whether its ability returns");
                }
                self.require(ability)?;
                // What the handler is at these type arguments.
                let handler = info.handler(info.ret.clone(), Vec::new());
                let handler = if types.is_empty() {
                    handler
                } else {
                    self.applied(&handler, types)?
                };
                let Ty::Function { params, ret, .. } = handler else {
                    return shape("a generic ability is performed at no type arguments");
                };
                for (arg, param) in args.iter().zip(&params) {
                    self.check(arg, param)?;
                }
                *ret
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
            let gives = if aborts {
                Ty::Unknown
            } else {
                info.ret.clone()
            };
            let expected = env.normalize(&info.handler(gives, Vec::new()));
            if aborts {
                // It runs at the provide, so it may take what is bound
                // there, and what it gives the provide gives.
                let Ty::Function { params, ret, row } = env.unfold(&found) else {
                    return Err(verifier.mismatch(&expected, &found));
                };
                if env.abort_handler(&info.params, &params) != Some(handler.traced) {
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
