//! Lambdas: each is a definition of its own, checked in its own frame.

use super::{Checker, Frame};
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// A lambda: its own definition, checked in a frame of its own.
    ///
    /// `hint` is the parameter types expected of it, if anything expects
    /// any. The result type is always the body's.
    pub(super) fn lambda(
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
}
