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
    /// them and for the tasks it starts.
    pub(super) fn primitive(
        &mut self,
        operation: core::Primitive,
        args: Vec<Value>,
        scope: &mut Scope,
    ) -> Op {
        if operation.copies() {
            self.charge_copies(&args, scope);
        }
        if operation.starts() && self.metered {
            for thunks in &args {
                self.bound(Op::ChargeTasks(*thunks), scope);
            }
        }
        // Tasks are started the one way, whether the functions were
        // written out or are the members of a list.
        match (operation, &args[..]) {
            (core::Primitive::ListAll, [thunks]) => Op::All(*thunks),
            (core::Primitive::ListFirst, [thunks]) => Op::First(*thunks),
            _ => Op::Primitive { operation, args },
        }
    }

    /// Charges for the function reading `key` of `record` makes if a
    /// method is held there, if this is metered code.
    pub(super) fn charge_read(&mut self, record: Value, key: usize, scope: &mut Scope) {
        if self.metered {
            self.bound(Op::ChargeRead { record, key }, scope);
        }
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
