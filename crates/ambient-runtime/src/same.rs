//! Whether two descriptors describe the same type.
//!
//! No value says what type it is but a cell, which keeps the type it was
//! made at. A cast to a cell type therefore compares two types
//! (`spec/backends.md`): a name stands for its definition, and a union
//! is its members in any order. This is no part of subtyping, which is
//! decided only where a program is checked.

use alloc::vec::Vec;

use crate::is::descriptor as d;
use crate::object::bytes;
use crate::scope::{At, Env, Scope, argument};

/// The most scopes one comparison enters. A regular type never comes
/// near it; it is what makes an irregular one fail rather than run
/// forever.
const DEPTH: usize = 256;

pub(crate) struct Types {
    scopes: Vec<Scope>,
    /// The scopes a comparison may enter before it fails.
    limit: usize,
    /// The pairs being compared, and those found the same under them.
    /// Meeting one again settles it: two recursive types are the same if
    /// nothing shows otherwise.
    assumed: Vec<(At, At)>,
}

unsafe fn word(node: u64, index: usize) -> u64 {
    unsafe { (node as usize as *const u64).add(index).read() }
}

/// `count` words of a node, from `start`.
unsafe fn span(node: u64, start: usize, count: u64) -> &'static [u64] {
    unsafe { core::slice::from_raw_parts((node as usize as *const u64).add(start), count as usize) }
}

impl Types {
    /// `scopes` are those already entered, which a node may be read in.
    pub(crate) fn new(scopes: &[Scope]) -> Self {
        Self {
            scopes: scopes.to_vec(),
            limit: scopes.len() + DEPTH,
            assumed: Vec::new(),
        }
    }

    /// The scope a generic definition is read in at these arguments. One
    /// entered before is found again, or a recursive generic type would
    /// never be met twice.
    fn enter(&mut self, nodes: &'static [u64], outer: Env) -> usize {
        let same = |scope: &Scope| core::ptr::eq(scope.nodes, nodes) && scope.outer == outer;
        self.scopes.iter().position(same).unwrap_or_else(|| {
            self.scopes.push(Scope { nodes, outer });
            self.scopes.len() - 1
        })
    }

    /// The node a name, an application, or a parameter stands for.
    unsafe fn head(&mut self, (mut ty, mut scope): At) -> At {
        unsafe {
            loop {
                match word(ty, 0) {
                    d::ALIAS => ty = word(ty, 1),
                    d::APPLY => {
                        let nodes = span(ty, 3, word(ty, 2));
                        scope = Env::Entered(self.enter(nodes, scope));
                        ty = word(ty, 1);
                    }
                    d::PARAMETER => {
                        let position = word(ty, 1) as usize;
                        let Some(found) = argument(&self.scopes, scope, position) else {
                            return (ty, scope);
                        };
                        (ty, scope) = found;
                    }
                    _ => return (ty, scope),
                }
            }
        }
    }

    /// Whether two types are one generic definition at the same
    /// arguments. A definition that applies itself to other types has
    /// no end of unfoldings, and this is how two uses of it are found
    /// the same.
    unsafe fn applied_alike(&mut self, (left, from): At, (right, to): At) -> bool {
        unsafe {
            let applied = |node: u64| word(node, 0) == d::APPLY;
            applied(left)
                && applied(right)
                && word(left, 1) == word(right, 1)
                && self.each(
                    (span(left, 3, word(left, 2)), from),
                    (span(right, 3, word(right, 2)), to),
                )
        }
    }

    /// Whether two types are the same.
    pub(crate) unsafe fn same(&mut self, left: At, right: At) -> bool {
        unsafe {
            if self.applied_alike(left, right) {
                return true;
            }
            let (left, right) = (self.head(left), self.head(right));
            if left == right || self.assumed.contains(&(left, right)) {
                return true;
            }
            if self.scopes.len() > self.limit {
                return false;
            }
            let depth = self.assumed.len();
            self.assumed.push((left, right));
            let holds = self.alike(left, right);
            if !holds {
                // What failed must not stay assumed.
                self.assumed.truncate(depth);
            }
            holds
        }
    }

    /// Whether `function` is the type of a function that steps an
    /// iterator of `item`: one that takes nothing, performs nothing, and
    /// gives null or a tuple of an item and an iterator of them.
    pub(crate) unsafe fn steps(&mut self, function: At, item: At) -> bool {
        unsafe {
            let (node, from) = self.head(function);
            if word(node, 0) != d::FUNCTION {
                return false;
            }
            let stepper = self::function(node);
            let (gives, from) = self.head((stepper.ret, from));
            if !(stepper.params.is_empty() && stepper.row.is_empty())
                || word(gives, 0) != d::UNION
                || word(gives, 1) != 2
            {
                return false;
            }
            // Null and the step, in either order.
            let members = span(gives, 2, 2);
            let is_null = |types: &mut Self, at: u64| word(types.head((at, from)).0, 0) == d::NULL;
            let step = match (is_null(self, members[0]), is_null(self, members[1])) {
                (true, false) => members[1],
                (false, true) => members[0],
                _ => return false,
            };
            let (step, from) = self.head((step, from));
            if word(step, 0) != d::TUPLE || word(step, 1) != 2 {
                return false;
            }
            let (rest, within) = self.head((word(step, 3), from));
            word(rest, 0) == d::ITERATOR
                && self.same((word(step, 2), from), item)
                && self.same((word(rest, 1), within), item)
        }
    }

    /// Whether each of `left`, read in a scope, is the same as the one
    /// of `right` beside it.
    unsafe fn each(&mut self, left: (&[u64], Env), right: (&[u64], Env)) -> bool {
        let ((left, from), (right, to)) = (left, right);
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(l, r)| unsafe { self.same((*l, from), (*r, to)) })
    }

    /// Whether each of `left` is the same as some one of `right`.
    unsafe fn among(&mut self, left: (&[u64], Env), right: (&[u64], Env)) -> bool {
        let ((left, from), (right, to)) = (left, right);
        left.iter().all(|l| {
            right
                .iter()
                .any(|r| unsafe { self.same((*l, from), (*r, to)) })
        })
    }

    /// [`Types::same`], of two nodes that stand for no other.
    unsafe fn alike(&mut self, (left, from): At, (right, to): At) -> bool {
        unsafe {
            let kind = word(left, 0);
            if kind != word(right, 0) {
                return false;
            }
            let members = |node: u64| span(node, 2, word(node, 1));
            match kind {
                d::UNION => {
                    let (left, right) = ((members(left), from), (members(right), to));
                    self.among(left, right) && self.among(right, left)
                }
                d::LITERAL => {
                    word(left, 1) == word(right, 1)
                        && match word(left, 1) {
                            d::LITERAL_STRING | d::LITERAL_BYTES => {
                                bytes(word(left, 2)) == bytes(word(right, 2))
                            }
                            _ => word(left, 2) == word(right, 2),
                        }
                }
                // Keys are interned, so one key is one word.
                d::RECORD => {
                    let fields = |node: u64| span(node, 2, word(node, 1) * 2);
                    let (have, want) = (fields(left), fields(right));
                    have.len() == want.len()
                        && want.chunks(2).all(|want| {
                            have.chunks(2).any(|have| {
                                have[0] == want[0] && self.same((have[1], from), (want[1], to))
                            })
                        })
                }
                d::TUPLE => self.each((members(left), from), (members(right), to)),
                d::LIST | d::CELL | d::ITERATOR => {
                    self.same((word(left, 1), from), (word(right, 1), to))
                }
                d::METHOD => {
                    self.same((word(left, 1), from), (word(right, 1), to))
                        && self.same((word(left, 2), from), (word(right, 2), to))
                }
                d::FUNCTION => {
                    let (have, want) = (function(left), function(right));
                    have.row == want.row
                        && self.each((have.params, from), (want.params, to))
                        && self.same((have.ret, from), (want.ret, to))
                }
                d::GENERIC => {
                    let after = |node: u64| word(node, 2 + word(node, 1) as usize);
                    self.each((members(left), from), (members(right), to))
                        && self.same((after(left), from), (after(right), to))
                }
                // The same parameter of the same generic function type:
                // the two are compared from the outside in.
                d::BOUND => word(left, 1) == word(right, 1),
                // Nothing is known of a parameter read outside any scope.
                d::PARAMETER => false,
                _ => true,
            }
        }
    }
}

/// A function type's node, read: see [`d::FUNCTION`].
struct Function {
    params: &'static [u64],
    ret: u64,
    /// The key of each ability it performs.
    row: &'static [u64],
}

unsafe fn function(node: u64) -> Function {
    unsafe {
        let params = span(node, 2, word(node, 1));
        let after = 2 + params.len();
        Function {
            params,
            ret: word(node, after),
            row: span(node, after + 2, word(node, after + 1)),
        }
    }
}
