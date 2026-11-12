//! Metering: the charges code makes when it runs inside a budget. See
//! `spec/budgets.md`.
//!
//! What a node costs is defined on the core tree ([`core::Expr::cost`]).
//! Lowering only places the charge: before the node, so that a budget
//! that cannot afford a node never begins it. Nothing lowering makes to
//! carry a node out is charged.

use super::{Lowering, Scope};
use crate::core;
use crate::lir::{Op, Value};

impl Lowering {
    /// Charges for `node`, if it costs anything and this is metered code.
    pub(super) fn charge(&mut self, node: &core::Expr, scope: &mut Scope) {
        let core::Cost { steps, words } = node.cost();
        if self.metered && (steps, words) != (0, 0) {
            self.bound(Op::Charge { steps, words }, scope);
        }
    }

    /// An operation on `args`, charged first for what it copies of
    /// them.
    pub(super) fn primitive(
        &mut self,
        operation: core::Primitive,
        args: Vec<Value>,
        scope: &mut Scope,
    ) -> Op {
        if operation.copies() {
            self.charge_copies(&args, scope);
        }
        Op::Primitive { operation, args }
    }

    /// Charges for a copy of each of `values`, if this is metered code:
    /// what merging them allocates.
    pub(super) fn charge_copies(&mut self, values: &[Value], scope: &mut Scope) {
        if self.metered {
            for value in values {
                self.bound(Op::ChargeSize(*value), scope);
            }
        }
    }
}
