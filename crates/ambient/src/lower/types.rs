//! Types at run time: testing a value against one, what a cell keeps
//! of the type it was made at, and the type arguments a generic function
//! is handed. See `spec/backends.md`.

use super::{Lowering, Scope};
use crate::core;
use crate::lir::{Constant, Expr, Op, Value};

impl Lowering {
    /// What a type written in the function being lowered is read in:
    /// the function's type arguments if the type mentions a parameter,
    /// and nothing if not.
    fn read_in(&self, ty: &core::Type) -> Value {
        if ty.mentions_parameter() {
            self.types
        } else {
            Value::Null
        }
    }

    /// A cell that holds `value`, which keeps the type it is made at.
    pub(super) fn cell(&mut self, value: Value, ty: &core::Type) -> Op {
        let scope = self.read_in(ty);
        let ty = self.constant(Constant::Type(ty.clone()));
        Op::Cell { value, ty, scope }
    }

    /// An expression that is true when `ty` describes `value`.
    pub(super) fn is(&mut self, value: Value, ty: &core::Type) -> Expr {
        let scope = self.read_in(ty);
        let ty = self.constant(Constant::Type(ty.clone()));
        Expr::Op(Op::Is { value, ty, scope })
    }

    /// The function in a slot, as a value: a closure that holds its type
    /// arguments, if it is generic.
    pub(super) fn reference(
        &mut self,
        slot: core::Slot,
        types: &[core::Type],
        scope: &mut Scope,
    ) -> Op {
        if types.is_empty() {
            return Op::Slot(slot.0);
        }
        Op::Closure {
            slot: slot.0,
            captures: vec![self.type_arguments(types, scope)],
        }
    }

    /// The type arguments a generic function is used at here, as the
    /// value a closure over it holds after what it captures.
    pub(super) fn type_arguments(&mut self, args: &[core::Type], scope: &mut Scope) -> Value {
        if !args.iter().any(core::Type::mentions_parameter) {
            return Value::Constant(self.constant(Constant::Types(args.to_vec())));
        }
        // Its own, handed on whole: what a function that calls itself or
        // a lambda inside it passes.
        let own = (0..self.type_params).map(core::Type::Parameter);
        if args.iter().eq(own.collect::<Vec<_>>().iter()) {
            return self.types;
        }
        let types = args
            .iter()
            .map(|ty| (self.constant(Constant::Type(ty.clone())), self.read_in(ty)))
            .collect();
        self.bound(Op::Types(types), scope)
    }
}
