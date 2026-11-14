//! Where each node of a tree was written.
//!
//! What the reader makes is often not shaped like what was written: a
//! statement becomes a binding around the rest of its block, and an `if`
//! a match. So it marks an expression as it reads it
//! ([`Expr::At`]), wherever the arranging then puts it. Once a
//! definition is read the marks are taken out and kept beside the tree:
//! a position for each node, in the order [`Expr::children`] walks them,
//! which is how a node is named to say an error is at it.

use super::Position;
use crate::surface::Expr;

/// Takes the marks out of `expr` and adds to `nodes` where each of its
/// nodes was written: where its own mark says, or where the node it is
/// inside of was. `tokens` is where each token the marks count is.
pub(super) fn lift(
    expr: &mut Expr,
    tokens: &[Position],
    inside: Position,
    nodes: &mut Vec<Position>,
) {
    let mut at = inside;
    // Of marks around marks, the innermost is nearest what was written.
    while let Expr::At(mark, marked) = expr {
        at = tokens.get(*mark).copied().unwrap_or(inside);
        *expr = std::mem::replace(&mut **marked, Expr::Null);
    }
    nodes.push(at);
    expr.children_mut(&mut |child| lift(child, tokens, at, nodes));
}
