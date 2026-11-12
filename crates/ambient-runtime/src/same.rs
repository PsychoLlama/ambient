//! Whether two descriptors describe one type.
//!
//! A cell is invariant in what it holds, so a cast to a cell type asks
//! whether the type the cell was made at is the type the cast names. The
//! two were written apart and are compared by structure: a name stands
//! for its definition, and a union is its members in any order.

use crate::is::{Scope, descriptor as d};
use crate::object::bytes;

/// A node, and the scope it is read in.
type At = (u64, Option<usize>);

pub(crate) struct Types {
    scopes: Vec<Scope>,
    /// The pairs being shown to be the same. Meeting one again shows it:
    /// a recursive type is the same as another if nothing tells them apart.
    assumed: Vec<(At, At)>,
}

impl Types {
    /// `scopes` are those already entered, which a node may be read in.
    pub(crate) fn new(scopes: &[Scope]) -> Self {
        Self {
            scopes: scopes.to_vec(),
            assumed: Vec::new(),
        }
    }

    /// The scope a generic definition is read in at these arguments. One
    /// entered before is found again, or a recursive generic type would
    /// never be met twice.
    fn enter(&mut self, nodes: &'static [u64], outer: Option<usize>) -> usize {
        let same = |scope: &Scope| std::ptr::eq(scope.nodes, nodes) && scope.outer == outer;
        self.scopes.iter().position(same).unwrap_or_else(|| {
            self.scopes.push(Scope { nodes, outer });
            self.scopes.len() - 1
        })
    }

    /// The node a name, an application, or a parameter stands for.
    unsafe fn head(&mut self, (mut ty, mut scope): At) -> At {
        unsafe {
            loop {
                let node = |index: usize| (ty as usize as *const u64).add(index).read();
                match node(0) {
                    d::ALIAS => ty = node(1),
                    d::APPLY => {
                        let arguments = (ty as usize as *const u64).add(3);
                        let nodes = std::slice::from_raw_parts(arguments, node(2) as usize);
                        scope = Some(self.enter(nodes, scope));
                        ty = node(1);
                    }
                    d::PARAMETER => {
                        let Some(inner) = scope else {
                            return (ty, scope);
                        };
                        let Scope { nodes, outer } = self.scopes[inner];
                        (ty, scope) = (nodes[node(1) as usize], outer);
                    }
                    _ => return (ty, scope),
                }
            }
        }
    }

    /// The members of a union, with the members of any union among them
    /// in its place; or the type alone, if it is not a union.
    unsafe fn members(&mut self, at: At, members: &mut Vec<At>) {
        unsafe {
            let (ty, scope) = self.head(at);
            let node = |index: usize| (ty as usize as *const u64).add(index).read();
            if node(0) != d::UNION {
                members.push((ty, scope));
                return;
            }
            for index in 0..node(1) as usize {
                self.members((node(2 + index), scope), members);
            }
        }
    }

    /// Whether two types are the same.
    pub(crate) unsafe fn same(&mut self, left: At, right: At) -> bool {
        unsafe {
            let (left, right) = (self.head(left), self.head(right));
            if left == right || self.assumed.contains(&(left, right)) {
                return true;
            }
            let depth = self.assumed.len();
            self.assumed.push((left, right));
            let (mut lefts, mut rights) = (Vec::new(), Vec::new());
            self.members(left, &mut lefts);
            self.members(right, &mut rights);
            let same = if let ([left], [right]) = (&lefts[..], &rights[..]) {
                self.alike(*left, *right)
            } else {
                // Each member of one is some member of the other.
                lefts
                    .iter()
                    .all(|l| rights.iter().any(|r| self.same(*l, *r)))
                    && rights
                        .iter()
                        .all(|r| lefts.iter().any(|l| self.same(*l, *r)))
            };
            if !same {
                // What failed must not stay assumed.
                self.assumed.truncate(depth);
            }
            same
        }
    }

    /// Whether two nodes, neither a union nor standing for another, are
    /// the same type.
    unsafe fn alike(&mut self, (left, left_scope): At, (right, right_scope): At) -> bool {
        unsafe {
            let l = |index: usize| (left as usize as *const u64).add(index).read();
            let r = |index: usize| (right as usize as *const u64).add(index).read();
            if l(0) != r(0) {
                return false;
            }
            let inside = |types: &mut Self, index: usize| {
                types.same((l(index), left_scope), (r(index), right_scope))
            };
            match l(0) {
                d::LITERAL => {
                    l(1) == r(1)
                        && match l(1) {
                            d::LITERAL_STRING => bytes(l(2)) == bytes(r(2)),
                            _ => l(2) == r(2),
                        }
                }
                // Fields are in one order everywhere, and keys are interned.
                d::RECORD => {
                    l(1) == r(1)
                        && (0..l(1) as usize).all(|field| {
                            let key = 2 + field * 2;
                            l(key) == r(key) && inside(self, key + 1)
                        })
                }
                d::TUPLE => l(1) == r(1) && (0..l(1) as usize).all(|i| inside(self, 2 + i)),
                d::LIST | d::CELL => inside(self, 1),
                // Nothing is known of a function's type, nor of a
                // parameter read outside any scope.
                d::FUNCTION | d::PARAMETER => false,
                _ => true,
            }
        }
    }
}
