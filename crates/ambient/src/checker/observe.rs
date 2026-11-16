//! Recording: an observer told of every ability a body performs. See
//! `spec/handlers.md`.
//!
//! A recording has no node of its own. For each ability its body takes
//! from outside, the checker derives a handler that forwards to the
//! binding outside and tells the observer, and provides those handlers
//! around the body.

use super::Checker;
use super::abilities::Layer;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Key, Reference};
use crate::symbol::Symbol;
use crate::types::{AbilityInfo, Ty, substitute};

/// What an observer is called with.
fn event() -> Ty {
    let field = |name: &str, ty| (Key::Field(name.to_owned()), ty);
    Ty::Record(vec![
        field("ability", Ty::String),
        field("arguments", Ty::List(Box::new(Ty::Unknown))),
        field("outcome", Ty::Unknown),
    ])
}

impl Checker<'_> {
    pub(super) fn observe(
        &mut self,
        observer: &Expr<Reference>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        // Whatever the observer takes it takes here, outside the
        // recording, so it never observes itself.
        let expected = Ty::Function {
            params: vec![event()],
            ret: Box::new(Ty::Unknown),
            row: Vec::new(),
        };
        let observer = self.check(observer, &expected)?;
        let number = self.next;
        self.bind_local(expected.clone());
        let depth = self.frames.len().saturating_sub(1);
        let held = self.reach(depth, number);

        self.frame().layers.push(Layer {
            abilities: Vec::new(),
            only: false,
            seen: Some(Vec::new()),
        });
        let body = self.infer(body);
        let seen = self.frame().layers.pop().and_then(|layer| layer.seen);
        let (body, ty) = body?;

        // The body's row: what it took from outside. In row order, so the
        // tree does not depend on the order the body used them in.
        let seen = seen.unwrap_or_default();
        let ordered: Vec<Symbol> = self
            .row(&seen)
            .iter()
            .map(|slot| self.frame().slots[slot.0].clone())
            .collect();
        let mut handlers = Vec::new();
        for ability in ordered {
            let Some(info) = self.env.abilities.get(&ability).cloned() else {
                continue;
            };
            handlers.push(core::Handler {
                ability: self.slot(&ability),
                handler: self.derive(&ability, &info, held.clone(), &expected)?,
                abort: (info.ret == Ty::Never).then_some(info.params.len()),
                traced: false,
            });
        }
        let provide = core::Expr::Provide {
            handlers,
            only: false,
            body: Box::new(body),
        };
        let core = core::Expr::Let {
            value: Box::new(observer),
            body: Box::new(provide),
        };
        Ok((core, ty))
    }

    /// The handler a recording provides for `ability`: it forwards to
    /// what is bound outside the recording, and tells the observer. It
    /// has no way to change the arguments or the result.
    fn derive(
        &mut self,
        ability: &Symbol,
        info: &AbilityInfo,
        observer: core::Expr,
        observer_ty: &Ty,
    ) -> Result<core::Expr, Error> {
        // It captures the binding it forwards to where it is made, which
        // is outside the recording.
        self.require(ability)?;
        // For a generic ability it is generic too: its type parameters
        // stand for the ability's, and it performs at them.
        let outer = self.env.parameters();
        let own: Vec<Ty> = (outer..outer + info.type_params.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        let at = |ty: &Ty| self.env.normalize(&substitute(ty, &own));
        let bounds: Vec<Ty> = info.type_params.iter().map(at).collect();
        let (params, ret): (Vec<Ty>, Ty) = (info.params.iter().map(at).collect(), at(&info.ret));
        let types = own.iter().map(|ty| self.lower(ty)).collect();
        self.env.inner.borrow_mut().extend(bounds);
        self.enter(Vec::new());
        self.frame().generic = own.len();
        self.frame().bindings = vec![ability.clone()];

        let ability = self.slot(ability);
        let arity = info.params.len();
        let args = || (0..arity).map(core::Expr::Local).collect::<Vec<_>>();
        let report = |outcome: core::Expr| {
            let field = |name: &str, value| (core::Key::Field(name.to_owned()), value);
            let event = core::Expr::Record(vec![
                field("ability", core::Expr::AbilityId(ability)),
                field("arguments", core::Expr::List(args())),
                field("outcome", outcome),
            ]);
            core::Expr::Call {
                callee: Box::new(core::Expr::Captured(0)),
                args: vec![event],
            }
        };
        let returns = info.ret != Ty::Never;
        let forward = core::Expr::Perform {
            ability,
            args: args(),
            returns,
            types,
        };
        let sequence = |first: core::Expr, then: core::Expr| core::Expr::Let {
            value: Box::new(first),
            body: Box::new(then),
        };
        let body = if returns {
            // The result is local `arity`: forward, report, give it back.
            let result = core::Expr::Local(arity);
            sequence(forward, sequence(report(result.clone()), result))
        } else {
            // Nothing comes back, so the observer is told first.
            sequence(report(core::Expr::Null), forward)
        };
        let made = self.leave(std::slice::from_ref(observer_ty), &params, &ret, body);
        let depth = outer - self.env.bounds.len();
        self.env.inner.borrow_mut().truncate(depth);
        Ok(made.closure(vec![observer]))
    }
}
