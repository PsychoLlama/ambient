//! Tasks, cells, and cleanups. See `spec/tasks.md`, `spec/cells.md`, and
//! `spec/cleanup.md`.

use super::Checker;
use super::abilities::Layer;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{CellOp, Expr, Key, Reference};
use crate::symbol::Symbol;
use crate::types::Ty;

/// What a boundary gives for a fault: an error, as `spec/errors.md`
/// shapes one. Nothing captures a trace yet, so it is always empty.
fn error() -> Ty {
    let field = |name: &str, ty| (Key::Field(name.to_owned()), ty);
    let frame = Ty::Record(vec![
        field("definition", Ty::String),
        field("position", Ty::Int),
    ]);
    Ty::Record(vec![
        field("kind", Ty::Tag),
        field("message", Ty::String),
        field("cause", Ty::Null),
        field("trace", Ty::List(Box::new(frame))),
    ])
}

impl Checker<'_> {
    /// A budget gives what its body does, or the limit that ran out, as
    /// a tag.
    pub(super) fn budget(
        &mut self,
        steps: &Expr<Reference>,
        allocation: &Expr<Reference>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let steps = self.check(steps, &Ty::Int)?;
        let allocation = self.check(allocation, &Ty::Int)?;
        let (body, ty) = self.infer(body)?;
        let node = core::Expr::Budget {
            steps: Box::new(steps),
            allocation: Box::new(allocation),
            body: Box::new(body),
        };
        Ok((node, self.env.union(vec![ty, Ty::Tag])))
    }

    pub(super) fn boundary(&mut self, body: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        let (body, ty) = self.infer(body)?;
        let ty = self.env.union(vec![ty, self.env.normalize(&error())]);
        Ok((core::Expr::Boundary(Box::new(body)), ty))
    }

    /// The branches of an all or a first, and what each gives. A branch
    /// runs as a task, and is a function that takes
    /// nothing, so it captures what it uses, bindings included, where the
    /// expression is written.
    fn tasks(&mut self, branches: &[Expr<Reference>]) -> Result<(Vec<core::Expr>, Vec<Ty>), Error> {
        let mut thunks = Vec::new();
        let mut results = Vec::new();
        for branch in branches {
            let (thunk, ty) = self.lambda(&[], &[], branch, None)?;
            thunks.push(thunk);
            results.push(match ty {
                Ty::Function { ret, .. } => *ret,
                _ => Ty::Never,
            });
        }
        Ok((thunks, results))
    }

    /// A cleanup is a function that takes nothing, made before the body
    /// runs: it has what is bound where it is registered, whatever the
    /// body's exit goes through. What it gives is dropped.
    pub(super) fn cleanup(
        &mut self,
        cleanup: &Expr<Reference>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (cleanup, _) = self.lambda(&[], &[], cleanup, None)?;
        let (body, ty) = self.infer(body)?;
        let node = core::Expr::Cleanup {
            cleanup: Box::new(cleanup),
            body: Box::new(body),
        };
        Ok((node, ty))
    }

    /// An all is the tuple of what its branches give; a first is any one
    /// of them.
    pub(super) fn join(
        &mut self,
        branches: &[Expr<Reference>],
        first: bool,
    ) -> Result<(core::Expr, Ty), Error> {
        let (thunks, results) = self.tasks(branches)?;
        Ok(if first {
            (core::Expr::First(thunks), self.env.union(results))
        } else {
            (core::Expr::All(thunks), Ty::Tuple(results))
        })
    }

    pub(super) fn group(&mut self, body: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        let depth = self.frame().layers.len();
        self.frame().groups.push(depth);
        let body = self.infer(body);
        self.frame().groups.pop();
        let (body, ty) = body?;
        Ok((core::Expr::Group(Box::new(body)), ty))
    }

    /// A spawn starts a task in the group it is written in. The task may
    /// outlast anything bound inside the group, so it is checked as if
    /// written where the group begins, and may use nothing bound since.
    pub(super) fn spawn(&mut self, task: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        let symbol = self.symbol.clone();
        let Some(depth) = self.frame().groups.last().copied() else {
            return Err(Error::SpawnOutsideGroup(symbol));
        };
        let inside = self.frame().layers.split_off(depth);
        // What is bound inside the group. A recording there stands
        // between the task and everything.
        let bound: Vec<Symbol> = inside
            .iter()
            .flat_map(|layer| layer.abilities.clone())
            .collect();
        let recorded = inside.iter().any(|layer| layer.seen.is_some());
        self.frame().layers.push(Layer {
            abilities: Vec::new(),
            only: inside.iter().any(|layer| layer.only),
            seen: Some(Vec::new()),
        });
        let thunk = self.lambda(&[], &[], task, None);
        let used = self.frame().layers.pop().and_then(|layer| layer.seen);
        self.frame().layers.extend(inside);

        let inner = |ability: &Symbol| recorded || bound.contains(ability);
        let refused = |ability: Symbol| Error::SpawnBinding { symbol, ability };
        match thunk {
            Err(Error::Unbound { ability, .. }) if inner(&ability) => Err(refused(ability)),
            Err(error) => Err(error),
            Ok((thunk, _)) => match used.unwrap_or_default().into_iter().find(inner) {
                Some(ability) => Err(refused(ability)),
                None => Ok((core::Expr::Spawn(Box::new(thunk)), Ty::Null)),
            },
        }
    }

    pub(super) fn cell(
        &mut self,
        operation: CellOp,
        ty: Option<&Ty>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let primitive = |operation, args| core::Expr::Primitive { operation, args };
        if let (CellOp::New, Some(ty), [value]) = (operation, ty, args) {
            self.well_formed(ty)?;
            let held = self.env.normalize(ty);
            let value = self.check(value, &held)?;
            let node = core::Expr::Cell {
                ty: self.lower(&held),
                value: Box::new(value),
            };
            return Ok((node, Ty::Cell(Box::new(held))));
        }
        // Every other operation takes a cell, and then values it could
        // hold.
        let Some((cell, values)) = args.split_first() else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let (cell, found) = self.infer(cell)?;
        let Ty::Cell(held) = self.env.unfold(&found) else {
            let expected = Ty::Cell(Box::new(Ty::Unknown));
            return Err(self.mismatch(&expected, &found));
        };
        let mut core_args = vec![cell];
        for value in values {
            core_args.push(self.check(value, &held)?);
        }
        let (operation, ty) = match operation {
            CellOp::New | CellOp::Read => (Primitive::CellRead, *held),
            CellOp::Replace => (Primitive::CellReplace, Ty::Bool),
            CellOp::Wait => (Primitive::CellWait, Ty::Null),
        };
        Ok((primitive(operation, core_args), ty))
    }
}
