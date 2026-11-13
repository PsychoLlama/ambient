//! Values written out: tuples, lists, and records.

use super::{Lowering, Scope, Unsupported};
use crate::core;
use crate::lir::Op;

impl Lowering {
    /// A tuple, a list, or a record, made of its members.
    pub(super) fn construction(
        &mut self,
        expr: &core::Expr,
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        Ok(match expr {
            core::Expr::Tuple(members) => Op::Sequence {
                list: false,
                members: self.operands(members, scope)?,
            },
            core::Expr::List(members) => Op::Sequence {
                list: true,
                members: self.operands(members, scope)?,
            },
            core::Expr::Record(fields) => {
                let mut lowered = Vec::new();
                for (key, value) in fields {
                    let value = self.operand(value, scope)?;
                    lowered.push((self.key(key), value));
                }
                Op::Record(lowered)
            }
            _ => return Err(Unsupported("a construction of what is not one")),
        })
    }
}
