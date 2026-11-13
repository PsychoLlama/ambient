//! Types at run time: testing a value against one, what a cell keeps
//! of the type it was made at, and the type arguments a generic function
//! is handed. See `spec/backends.md`.

use super::{Lowering, Scope, Unsupported};
use crate::core;
use crate::lir::{Constant, Expr, Op, Value};

/// What lowering a function takes besides its tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Form {
    /// Whether the function charges for what it does: see
    /// `spec/budgets.md`.
    pub metered: bool,
    /// Whether the function is handed its type arguments, which is to
    /// say whether it needs them as it runs.
    pub handed: bool,
    /// The same of the function in each slot. A slot past the end is
    /// one that is not.
    pub slots: Vec<bool>,
    /// Whether the function is part of a program in which nothing
    /// suspends, so that it never has to resume: see "Floor" in
    /// `spec/linking.md`.
    pub still: bool,
}

impl Form {
    /// The form of the same function that makes no charge.
    #[must_use]
    pub fn plain(&self) -> Self {
        Self {
            metered: false,
            ..self.clone()
        }
    }
}

impl Lowering {
    /// Whether a use of the function in `slot` at `types` hands it type
    /// arguments: it is generic, and it needs them.
    pub(super) fn hands(&self, slot: core::Slot, types: &[core::Type]) -> bool {
        !types.is_empty() && self.handed.get(slot.0).copied().unwrap_or(false)
    }

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

    /// A generic function at type arguments that mention no parameter:
    /// one closure, made when the program is loaded.
    pub(super) fn instance(&mut self, slot: core::Slot, types: &[core::Type]) -> Option<Value> {
        let closed = self.hands(slot, types) && !types.iter().any(core::Type::mentions_parameter);
        let instance = Constant::Instance(slot.0, types.to_vec());
        closed.then(|| Value::Constant(self.constant(instance)))
    }

    /// The function in a slot, as a value: a closure that holds its type
    /// arguments, if it is handed them.
    pub(super) fn reference(
        &mut self,
        slot: core::Slot,
        types: &[core::Type],
        scope: &mut Scope,
    ) -> Op {
        if !self.hands(slot, types) {
            return Op::Slot(slot.0);
        }
        if let Some(instance) = self.instance(slot, types) {
            return Op::Copy(instance);
        }
        Op::Closure {
            slot: slot.0,
            captures: vec![self.type_arguments(types, scope)],
        }
    }

    /// A generic function value at type arguments. What it is a
    /// closure over is not known here, so the runtime sees whether its
    /// function needs them: one that does is copied to hold them.
    pub(super) fn instantiate(
        &mut self,
        function: &core::Expr,
        types: &[core::Type],
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let function = self.operand(function, scope)?;
        self.charge_copies(&[function], scope);
        let types = self.type_arguments(types, scope);
        Ok(Op::Instantiate { function, types })
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
