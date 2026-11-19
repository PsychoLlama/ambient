//! Lambdas: each is a definition of its own, checked in its own frame.

use super::abilities::Layer;
use super::{Checker, Frame};
use crate::core::{self, Open, Slot, Tree};
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, abilities, generalize, instantiate};

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
    /// any, and `asked` the row: one that states no row takes that from
    /// its caller, and captures the rest as any lambda does. The result
    /// type is the one it states, or else the body's. One with type
    /// parameters is of a generic function type.
    pub(super) fn lambda(
        &mut self,
        lambda: &Expr<Reference>,
        hint: Option<&[Ty]>,
        asked: &[Reference],
    ) -> Result<(core::Expr, Ty), Error> {
        let Expr::Lambda { type_params, .. } = lambda else {
            return self.infer(lambda);
        };
        // Its type parameters are in scope for everything written in
        // it, a bound beside them included, and for nothing after it.
        let outer = self.env.parameters();
        let depth = outer - self.env.bounds.len();
        let unbounded = type_params.iter().map(|_| Ty::Unknown);
        self.env.inner.borrow_mut().extend(unbounded);
        let checked = self.bounds_of(lambda).and_then(|bounds| {
            self.env.inner.borrow_mut().splice(depth.., bounds.clone());
            let made = self.written(lambda, hint, asked)?;
            Ok((made, bounds))
        });
        self.env.inner.borrow_mut().truncate(depth);
        let ((closure, function), bounds) = checked?;
        if bounds.is_empty() {
            return Ok((closure, function));
        }
        let own = bounds.len();
        let generic = Ty::Generic {
            bounds: bounds.iter().map(|b| generalize(b, outer, own)).collect(),
            function: Box::new(generalize(&function, outer, own)),
        };
        Ok((closure, generic))
    }

    /// The bound of each type parameter a lambda has of its own.
    fn bounds_of(&self, lambda: &Expr<Reference>) -> Result<Vec<Ty>, Error> {
        let Expr::Lambda { type_params, .. } = lambda else {
            return Ok(Vec::new());
        };
        let mut bounds = Vec::new();
        for param in type_params {
            bounds.push(match &param.bound {
                Some(bound) => {
                    self.well_formed(bound)?;
                    self.env.normalize(bound)
                }
                None => Ty::Unknown,
            });
        }
        Ok(bounds)
    }

    /// The parameter types a generic lambda takes from a generic
    /// function type expected of it: those of the function type, with
    /// the lambda's own type parameters standing for the type's.
    pub(super) fn hinted(&self, bounds: &[Ty], function: &Ty) -> Option<Vec<Ty>> {
        let outer = self.env.parameters();
        let own: Vec<Ty> = (outer..outer + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        match self.env.normalize(&instantiate(function, &own)) {
            Ty::Function { params, .. } => Some(params.into()),
            _ => None,
        }
    }

    /// A lambda checked with its own type parameters in scope: the
    /// closure, and the type of its function as written inside it.
    fn written(
        &mut self,
        lambda: &Expr<Reference>,
        hint: Option<&[Ty]>,
        asked: &[Reference],
    ) -> Result<(core::Expr, Ty), Error> {
        let Expr::Lambda {
            type_params,
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
        let row = if row.is_empty() { asked } else { row };
        self.function_of(type_params.len(), types, row, returns, body)
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
        self.function_of(0, types, row, returns, body)
    }

    /// [`Self::function`], of a function that has `generic` type
    /// parameters of its own, the last that are in scope.
    fn function_of(
        &mut self,
        generic: usize,
        types: Vec<Ty>,
        row: &[Reference],
        returns: Option<Ty>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        self.enter(abilities(row));
        self.frame().generic = generic;
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
            params: types.into(),
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
        let bounds = env.all_bounds();
        let type_params = bounds.iter().map(|ty| self.lower(ty)).collect();
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
        let mut body = body;
        self.origins.insert(frame.path.clone(), body.origins(0));
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
            annotations: Vec::new(),
        });
        Made {
            slot: self.slot(&symbol),
            // A lambda has its encloser's type parameters, at themselves.
            // Any of its own are given where it is instantiated.
            type_args: (0..bounds.len() - frame.generic)
                .map(core::Type::Parameter)
                .collect(),
            // The same abilities, as the encloser's slots.
            bindings: self.row(&taken),
        }
    }
}
