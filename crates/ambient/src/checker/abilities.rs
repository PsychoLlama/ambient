//! Abilities: what is bound where, performs, provides, and binding a
//! function value in place. See `spec/abilities.md` and `spec/handlers.md`.

use super::Checker;
use crate::core::{self, Slot};
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, abilities};

/// Abilities bound for part of a definition: by its row, or by a provide.
pub(super) struct Layer {
    pub(super) abilities: Vec<Symbol>,
    /// Whether nothing bound outside this layer can be seen inside it.
    pub(super) only: bool,
}

impl Checker<'_> {
    /// Fails unless `ability` is bound where checking is now.
    pub(super) fn require(&mut self, ability: &Symbol) -> Result<(), Error> {
        let depth = self.frames.len().saturating_sub(1);
        self.require_at(depth, ability)
    }

    pub(super) fn require_all(&mut self, row: &[Reference]) -> Result<(), Error> {
        abilities(row)
            .iter()
            .try_for_each(|ability| self.require(ability))
    }

    /// Fails unless `ability` is bound in the frame at `depth`. If only a
    /// frame outside binds it, each lambda in between captures it.
    fn require_at(&mut self, depth: usize, ability: &Symbol) -> Result<(), Error> {
        let unbound = || Error::Unbound {
            symbol: self.symbol.clone(),
            ability: ability.clone(),
        };
        let Some(frame) = self.frames.get(depth) else {
            return Err(unbound());
        };
        for layer in frame.layers.iter().rev() {
            if layer.abilities.contains(ability) {
                return Ok(());
            }
            if layer.only {
                return Err(unbound());
            }
        }
        if frame.bindings.contains(ability) {
            return Ok(());
        }
        if depth == 0 {
            return Err(unbound());
        }
        self.require_at(depth - 1, ability)?;
        self.frames[depth].bindings.push(ability.clone());
        Ok(())
    }

    /// A row as slots, ordered by tag: the one order that depends on
    /// neither names nor where the row was written.
    pub(super) fn row(&mut self, row: &[Symbol]) -> Vec<Slot> {
        let env = self.env;
        let tag = |ability: &Symbol| env.abilities.get(ability).map(|info| &info.tag);
        let mut row = row.to_vec();
        row.sort_by(|a, b| tag(a).cmp(&tag(b)).then_with(|| a.cmp(b)));
        row.dedup();
        row.iter().map(|ability| self.slot(ability)).collect()
    }

    pub(super) fn perform(
        &mut self,
        ability: &Reference,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        // Resolution only ever names an ability here.
        let Reference::Definition(ability) = ability else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let Some(info) = self.env.abilities.get(ability).cloned() else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        if info.params.len() != args.len() {
            return Err(Error::Arity {
                symbol: self.symbol.clone(),
                callee: ability.clone(),
                expected: info.params.len(),
                found: args.len(),
            });
        }
        self.require(ability)?;
        let mut core_args = Vec::new();
        for (arg, param) in args.iter().zip(&info.params) {
            core_args.push(self.check(arg, param)?);
        }
        let core = core::Expr::Perform {
            ability: self.slot(ability),
            args: core_args,
            returns: info.ret != Ty::Never,
        };
        Ok((core, info.ret))
    }

    /// A provide: handlers are checked where the provide is written, and
    /// the body where they are bound.
    pub(super) fn provide(
        &mut self,
        handlers: &[(Reference, Expr<Reference>)],
        only: bool,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let mut bound = Vec::new();
        let mut core_handlers = Vec::new();
        // What the provide may evaluate to besides its body: the result of
        // each handler that aborts to it.
        let mut results = Vec::new();
        for (ability, handler) in handlers {
            let Reference::Definition(ability) = ability else {
                continue;
            };
            let Some(info) = self.env.abilities.get(ability).cloned() else {
                continue;
            };
            let (handler, abort) = if info.ret == Ty::Never {
                let (handler, ret) = self.abort_handler(handler, &info.params)?;
                results.push(ret);
                (handler, Some(info.params.len()))
            } else {
                // Bound in place: whatever the handler takes from its
                // caller it takes here, so a perform passes it nothing.
                let expected = Ty::Function {
                    params: info.params.clone(),
                    ret: Box::new(info.ret.clone()),
                    row: Vec::new(),
                };
                (self.check(handler, &expected)?, None)
            };
            core_handlers.push(core::Handler {
                ability: self.slot(ability),
                handler,
                abort,
            });
            bound.push(ability.clone());
        }

        self.frame().layers.push(Layer {
            abilities: bound,
            only,
        });
        let body = self.infer(body);
        self.frame().layers.pop();
        let (body, ty) = body?;
        results.push(ty);
        let core = core::Expr::Provide {
            handlers: core_handlers,
            only,
            body: Box::new(body),
        };
        Ok((core, self.env.union(results)))
    }

    /// A handler for an ability that does not return. It runs at the
    /// provide, so it may take any ability bound there and return anything.
    fn abort_handler(
        &mut self,
        handler: &Expr<Reference>,
        params: &[Ty],
    ) -> Result<(core::Expr, Ty), Error> {
        let expected = Ty::Function {
            params: params.to_vec(),
            ret: Box::new(Ty::Unknown),
            row: Vec::new(),
        };
        let (core, found) = self.infer_as(handler, &expected)?;
        let Ty::Function {
            params: takes,
            ret,
            row,
        } = self.env.unfold(&found)
        else {
            return Err(self.mismatch(&expected, &found));
        };
        let fits = takes.len() == params.len()
            && params
                .iter()
                .zip(&takes)
                .all(|(given, taken)| self.env.subtype(given, taken));
        if !fits {
            return Err(self.mismatch(&expected, &found));
        }
        self.require_all(&row)?;
        Ok((core, *ret))
    }

    /// A function value at the row expected of it. Abilities it takes
    /// beyond that row are bound here: the result is a closure that
    /// captures them and the value.
    pub(super) fn bind_row(
        &mut self,
        value: core::Expr,
        found: Ty,
        expected: &Ty,
    ) -> Result<(core::Expr, Ty), Error> {
        let (Ty::Function { params, ret, row }, Ty::Function { row: wanted, .. }) =
            (self.env.unfold(&found), self.env.unfold(expected))
        else {
            return Ok((value, found));
        };
        let (kept, excess): (Vec<_>, Vec<_>) =
            row.iter().cloned().partition(|a| wanted.contains(a));
        if excess.is_empty() {
            return Ok((value, found));
        }
        self.require_all(&excess)?;

        self.enter(abilities(&kept));
        self.frame().bindings = abilities(&excess);
        let body = core::Expr::Call {
            callee: Box::new(core::Expr::Captured(0)),
            args: (0..params.len()).map(core::Expr::Local).collect(),
        };
        let made = self.leave(std::slice::from_ref(&found), &params, &ret, body);
        let ty = Ty::Function {
            params,
            ret,
            row: kept,
        };
        Ok((made.closure(vec![value]), ty))
    }
}
