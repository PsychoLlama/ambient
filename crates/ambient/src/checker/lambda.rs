//! Lambdas: each is a definition of its own, checked in its own frame.

use super::abilities::Layer;
use super::{Checker, Frame};
use crate::core::{self, Open, Slot, Tree};
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, abilities};

/// A definition made inside another, as the encloser refers to it.
pub(super) struct Made {
    slot: Slot,
    type_args: Vec<core::Type>,
    bindings: Vec<Slot>,
}

impl Made {
    /// A closure over the definition and the values it captures.
    pub(super) fn closure(self, captures: Vec<core::Expr>) -> core::Expr {
        core::Expr::Closure {
            slot: self.slot,
            type_args: self.type_args,
            captures,
            bindings: self.bindings,
        }
    }
}

impl Checker<'_> {
    /// A lambda: its own definition, checked in a frame of its own.
    ///
    /// `hint` is the parameter types expected of it, if anything expects
    /// any. The result type is the one it states, or else the body's.
    pub(super) fn lambda(
        &mut self,
        lambda: &Expr<Reference>,
        hint: Option<&[Ty]>,
    ) -> Result<(core::Expr, Ty), Error> {
        let Expr::Lambda {
            params,
            row,
            returns,
            body,
        } = lambda
        else {
            return self.infer(lambda);
        };
        let returns = match returns {
            Some(ty) => {
                self.well_formed(ty)?;
                Some(self.env.normalize(ty))
            }
            None => None,
        };
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
        self.function(types, row, returns, body)
    }

    /// A function written in place: one that takes `types` and its
    /// caller's `row`, and gives `returns` if that is stated.
    pub(super) fn function(
        &mut self,
        types: Vec<Ty>,
        row: &[Reference],
        returns: Option<Ty>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        self.enter(abilities(row));
        for ty in &types {
            self.bind_local(ty.clone());
        }
        let (body, ret) = match returns {
            Some(ret) => (self.check(body, &ret)?, ret),
            None => self.infer(body)?,
        };
        let outer = self.frame().captures.clone();
        let held: Vec<Ty> = outer
            .iter()
            .map(|number| self.types.get(number).cloned().unwrap_or(Ty::Never))
            .collect();
        let made = self.leave(&held, &types, &ret, body);

        // The encloser hands over each captured value, capturing it in turn
        // if it is not its own.
        let depth = self.frames.len() - 1;
        let captures = outer
            .iter()
            .map(|number| self.reach(depth, *number))
            .collect();
        let mut row = row.to_vec();
        row.sort();
        row.dedup();
        let ty = Ty::Function {
            params: types,
            ret: Box::new(ret),
            row,
        };
        Ok((made.closure(captures), ty))
    }

    /// Starts a definition inside the current one. `row` is what it takes
    /// from its caller.
    pub(super) fn enter(&mut self, row: Vec<Symbol>) {
        let mut path = self.frame().path.clone();
        path.push(self.frame().lambdas.len());
        self.frames.push(Frame {
            path,
            layers: vec![Layer {
                abilities: row,
                only: false,
                seen: None,
            }],
            ..Frame::default()
        });
    }

    /// Finishes the definition [`Self::enter`] started, given the types of
    /// the values it captures, its parameters, its result, and its body.
    pub(super) fn leave(
        &mut self,
        values: &[Ty],
        params: &[Ty],
        ret: &Ty,
        body: core::Expr,
    ) -> Made {
        // The definition's own tree is lowered while its frame is current,
        // so the slots land in its table.
        let env = self.env;
        let type_params = env.bounds.iter().map(|ty| self.lower(ty)).collect();
        let captures = values.iter().map(|ty| self.lower(ty)).collect();
        let params = params.iter().map(|ty| self.lower(ty)).collect();
        let ret = self.lower(ret);
        let taken = self.frame().bindings.clone();
        let row = match self.frame().layers.first() {
            Some(layer) => layer.abilities.clone(),
            None => Vec::new(),
        };
        let bindings = self.row(&taken);
        let row = self.row(&row);
        let frame = self.frames.pop().unwrap_or_default();
        let symbol = self.symbol.lambda(&frame.path);
        self.frame().lambdas.push(Open {
            tree: Tree::Function(core::Function {
                type_params,
                captures,
                bindings,
                params,
                ret,
                row,
                body,
            }),
            slots: frame.slots,
            lambdas: frame.lambdas,
        });
        Made {
            slot: self.slot(&symbol),
            // A lambda has its encloser's type parameters, at themselves.
            type_args: (0..env.bounds.len()).map(core::Type::Parameter).collect(),
            // The same abilities, as the encloser's slots.
            bindings: self.row(&taken),
        }
    }
}
