//! Abilities: what is bound where, performs, provides, and binding a
//! function value in place. See `spec/abilities.md` and `spec/handlers.md`.

use super::Checker;
use crate::core::{self, Slot};
use crate::error::Error;
use crate::surface::{Expr, Key, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, TyKey, abilities, generalize, instantiate};

/// Abilities bound for part of a definition: by its row, or by a provide.
pub(super) struct Layer {
    pub(super) abilities: Vec<Symbol>,
    /// Whether nothing bound outside this layer can be seen inside it.
    pub(super) restricted: bool,
    /// For a recording: every ability that was looked for inside it and
    /// found only outside it.
    pub(super) seen: Option<Vec<Symbol>>,
}

impl Checker<'_> {
    /// Fails unless `ability` is bound where checking is now.
    pub(super) fn require(&mut self, ability: &Symbol) -> Result<(), Error> {
        if self.providing {
            return Ok(());
        }
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
        let symbol = self.symbol;
        let unbound = || Error::Unbound {
            symbol: symbol.clone(),
            ability: ability.clone(),
        };
        let Some(frame) = self.frames.get_mut(depth) else {
            return Err(unbound());
        };
        for layer in frame.layers.iter_mut().rev() {
            if layer.abilities.contains(ability) {
                return Ok(());
            }
            if layer.restricted {
                return Err(unbound());
            }
            if let Some(seen) = &mut layer.seen
                && !seen.contains(ability)
            {
                seen.push(ability.clone());
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
        let row = self.env.row(row);
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
        // An ability whose signature is generic is performed at
        // whatever type arguments its arguments require.
        if !info.type_params.is_empty() {
            let signature = info.signature();
            let solved = self.generic_arguments(Some(ability), &signature, 0, None, args)?;
            let types = solved.types.iter().map(|ty| self.lower(ty)).collect();
            let core = core::Expr::Perform {
                ability: self.slot(ability),
                args: solved.args,
                returns: true,
                types,
            };
            return Ok((core, solved.ret));
        }
        let mut core_args = Vec::new();
        for (arg, param) in args.iter().zip(&info.params) {
            core_args.push(self.check(arg, param)?);
        }
        let core = core::Expr::Perform {
            ability: self.slot(ability),
            args: core_args,
            returns: info.ret != Ty::Never,
            types: Vec::new(),
        };
        Ok((core, info.ret))
    }

    /// A provide: the bundle is evaluated and its handlers bound where the
    /// provide is written, and the body is checked where they are bound.
    pub(super) fn provide(
        &mut self,
        bundle: &Expr<Reference>,
        restricted: bool,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (bundle, ty) = self.infer(bundle)?;
        let not_bundle = Error::NotBundle {
            symbol: self.symbol.clone(),
            found: self.env.show(&ty),
        };
        let Ty::Record(fields) = self.env.unfold(&ty) else {
            return Err(not_bundle);
        };
        // The bundle is held in a local while its handlers are read. Only
        // what its type lists is bound, whatever else it holds.
        let number = self.next;
        self.bind_local(ty);
        let depth = self.frames.len().saturating_sub(1);
        let held = self.reach(depth, number);
        let entries: Vec<(TyKey, Ty)> = self
            .env
            .ordered(&fields)
            .into_iter()
            .map(|(key, ty)| (key.clone(), ty.clone()))
            .collect();

        let mut bound = Vec::new();
        let mut handlers = Vec::new();
        // What the provide may evaluate to besides its body: the result of
        // each handler that aborts to it.
        let mut results = Vec::new();
        for (key, found) in entries {
            let Key::Ability(Reference::Definition(ability)) = &key else {
                return Err(not_bundle);
            };
            let Some(info) = self.env.abilities.get(ability).cloned() else {
                continue;
            };
            let read = core::Expr::Field {
                target: Box::new(held.clone()),
                key: self.key(&key),
            };
            let mut traced = false;
            let (handler, abort) = if info.ret == Ty::Never {
                let (gives, takes_trace) = self.abort_handler(&found, &info.params)?;
                results.push(gives);
                traced = takes_trace;
                (read, Some(info.params.len()))
            } else {
                // Bound in place: whatever the handler takes from its
                // caller it takes here, so a perform passes it nothing.
                let expected = info.handler(info.ret.clone(), Vec::new());
                let (handler, found) = self.bind_row(read, found, &expected)?;
                if !self.env.subtype(&found, &expected) {
                    return Err(self.mismatch(&expected, &found));
                }
                (handler, None)
            };
            handlers.push(core::Handler {
                ability: self.slot(ability),
                handler,
                abort,
                traced,
            });
            bound.push(ability.clone());
        }

        self.frame().layers.push(Layer {
            abilities: bound,
            restricted,
            seen: None,
        });
        let body = self.infer(body);
        self.frame().layers.pop();
        let (body, ty) = body?;
        results.push(ty);
        let provide = core::Expr::Provide {
            handlers,
            restricted,
            body: Box::new(body),
        };
        let core = core::Expr::Let {
            value: Box::new(bundle),
            body: Box::new(provide),
        };
        Ok((core, self.env.union(results)))
    }

    /// What a handler of type `found`, for an ability that does not
    /// return, makes its provide evaluate to. It runs at the provide, so
    /// it may take any ability bound there and return anything. Also
    /// whether it takes the trace of the abort: see
    /// [`Env::abort_handler`].
    fn abort_handler(&mut self, found: &Ty, params: &[Ty]) -> Result<(Ty, bool), Error> {
        let expected = Ty::Function {
            params: params.to_vec().into(),
            ret: Box::new(Ty::Unknown),
            row: Vec::new(),
        };
        let Ty::Function {
            params: takes,
            ret,
            row,
        } = self.env.unfold(found)
        else {
            return Err(self.mismatch(&expected, found));
        };
        let Some(traced) = self.env.abort_handler(params, &takes) else {
            return Err(self.mismatch(&expected, found));
        };
        self.require_all(&row)?;
        Ok((*ret, traced))
    }

    /// A function value at the row expected of it. Abilities it takes
    /// beyond that row are bound here: the result is a closure that
    /// captures them and the value. A generic function expected to be
    /// one with as many type parameters is bound the same way, by a
    /// closure that has them too.
    pub(super) fn bind_row(
        &mut self,
        value: core::Expr,
        found: Ty,
        expected: &Ty,
    ) -> Result<(core::Expr, Ty), Error> {
        let outer = self.env.parameters();
        let parts = |ty: &Ty| match self.env.unfold(ty) {
            Ty::Generic { bounds, function } => (bounds, *function),
            other => (Vec::new(), other),
        };
        let ((bounds, written), (wanted_bounds, wanted)) = (parts(&found), parts(expected));
        if bounds.len() != wanted_bounds.len() {
            return Ok((value, found));
        }
        let own: Vec<Ty> = (outer..outer + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        // What is not generic is already as it will be read.
        let open = |ty: &Ty| match own.len() {
            0 => ty.clone(),
            _ => self.env.normalize(&instantiate(ty, &own)),
        };
        let (Ty::Function { params, ret, row }, Ty::Function { row: wanted, .. }) =
            (open(&written), open(&wanted))
        else {
            return Ok((value, found));
        };
        let (kept, excess): (Vec<_>, Vec<_>) =
            row.iter().cloned().partition(|a| wanted.contains(a));
        if excess.is_empty() {
            return Ok((value, found));
        }
        self.require_all(&excess)?;

        let bounds: Vec<Ty> = bounds.iter().map(open).collect();
        self.env.inner.borrow_mut().extend(bounds.iter().cloned());
        self.enter(abilities(&kept));
        self.frame().generic = own.len();
        self.frame().bindings = abilities(&excess);
        let callee = match own.len() {
            0 => core::Expr::Captured(0),
            _ => self.instantiated(core::Expr::Captured(0), &own),
        };
        let body = core::Expr::Call {
            callee: Box::new(callee),
            args: (0..params.len()).map(core::Expr::Local).collect(),
        };
        let made = self.leave(std::slice::from_ref(&found), &params, &ret, body);
        let depth = outer - self.env.bounds.len();
        self.env.inner.borrow_mut().truncate(depth);
        let function = Ty::Function {
            params,
            ret,
            row: kept,
        };
        let close = |ty: &Ty| generalize(ty, outer, own.len());
        let ty = match own.len() {
            0 => function,
            _ => Ty::Generic {
                bounds: bounds.iter().map(close).collect(),
                function: Box::new(close(&function)),
            },
        };
        Ok((made.closure(vec![value]), ty))
    }
}
