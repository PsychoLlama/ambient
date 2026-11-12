//! How two descriptors relate: whether every value one describes is a
//! value the other describes.
//!
//! No value says what type it is but a cell, which keeps the type it was
//! made at, and a function, whose definition holds its signature. A cast
//! to a cell type or a function type therefore compares two types, as
//! checking does (`spec/types.md`): a name stands for its definition, and
//! a union is its members in any order.

use crate::is::descriptor as d;
use crate::object::bytes;
use crate::scope::{At, Env, Scope, argument};

/// The most scopes one comparison enters. A regular type never comes
/// near it; it is what makes an irregular one fail rather than run
/// forever.
const DEPTH: usize = 256;

/// The two booleans, as the literal types a boolean is the union of.
static BOOLEANS: [[u64; 3]; 2] = [
    [d::LITERAL, d::LITERAL_BOOL, 0],
    [d::LITERAL, d::LITERAL_BOOL, 1],
];

pub(crate) struct Types {
    scopes: Vec<Scope>,
    /// The scopes a comparison may enter before it fails.
    limit: usize,
    /// The judgements being proved, and those proved under them. Meeting
    /// one again proves it: a recursive type is within another if nothing
    /// shows otherwise.
    assumed: Vec<(At, At)>,
}

unsafe fn word(node: u64, index: usize) -> u64 {
    unsafe { (node as usize as *const u64).add(index).read() }
}

/// `count` words of a node, from `start`.
unsafe fn span(node: u64, start: usize, count: u64) -> &'static [u64] {
    unsafe { std::slice::from_raw_parts((node as usize as *const u64).add(start), count as usize) }
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

    /// Whether two types are the same: each is within the other.
    pub(crate) unsafe fn same(&mut self, left: At, right: At) -> bool {
        unsafe { self.within(left, right) && self.within(right, left) }
    }

    /// Whether every value `sub` describes is one `sup` describes.
    pub(crate) unsafe fn within(&mut self, sub: At, sup: At) -> bool {
        unsafe {
            let (sub, sup) = (self.head(sub), self.head(sup));
            if sub == sup || self.assumed.contains(&(sub, sup)) {
                return true;
            }
            if self.scopes.len() > self.limit {
                return false;
            }
            let depth = self.assumed.len();
            self.assumed.push((sub, sup));
            let holds = self.related(sub, sup);
            if !holds {
                // What failed must not stay assumed.
                self.assumed.truncate(depth);
            }
            holds
        }
    }

    /// Whether each of `lower`, read in a scope, is within the one of
    /// `upper` beside it.
    unsafe fn each(&mut self, lower: (&[u64], Env), upper: (&[u64], Env)) -> bool {
        let ((lower, from), (upper, to)) = (lower, upper);
        lower.len() == upper.len()
            && lower
                .iter()
                .zip(upper)
                .all(|(below, above)| unsafe { self.within((*below, from), (*above, to)) })
    }

    /// [`Types::within`], of two nodes that stand for no other.
    unsafe fn related(&mut self, (sub, from): At, (sup, to): At) -> bool {
        unsafe {
            let (below, above) = (word(sub, 0), word(sup, 0));
            let members = |node: u64| span(node, 2, word(node, 1));
            if below == d::NEVER || above == d::UNKNOWN {
                return true;
            }
            if below == d::UNION {
                return members(sub)
                    .iter()
                    .all(|member| self.within((*member, from), (sup, to)));
            }
            if above == d::UNION {
                // A boolean is true or false, which a union may hold apart.
                if below == d::BOOL {
                    return BOOLEANS.iter().all(|literal| {
                        self.within((literal.as_ptr() as u64, Env::None), (sup, to))
                    });
                }
                return members(sup)
                    .iter()
                    .any(|member| self.within((sub, from), (*member, to)));
            }
            match (below, above) {
                (d::LITERAL, d::LITERAL) => {
                    word(sub, 1) == word(sup, 1)
                        && match word(sub, 1) {
                            d::LITERAL_STRING => bytes(word(sub, 2)) == bytes(word(sup, 2)),
                            _ => word(sub, 2) == word(sup, 2),
                        }
                }
                (d::LITERAL, _) => {
                    above
                        == match word(sub, 1) {
                            d::LITERAL_BOOL => d::BOOL,
                            d::LITERAL_INT => d::INT,
                            d::LITERAL_FLOAT => d::FLOAT,
                            d::LITERAL_STRING => d::STRING,
                            _ => d::TAG,
                        }
                }
                // Keys are interned, so one key is one word.
                (d::RECORD, d::RECORD) => {
                    let have = span(sub, 2, word(sub, 1) * 2);
                    span(sup, 2, word(sup, 1) * 2).chunks(2).all(|want| {
                        have.chunks(2).any(|have| {
                            have[0] == want[0] && self.within((have[1], from), (want[1], to))
                        })
                    })
                }
                (d::TUPLE, d::TUPLE) => self.each((members(sub), from), (members(sup), to)),
                // A tuple is a list of its members.
                (d::TUPLE, d::LIST) => members(sub)
                    .iter()
                    .all(|member| self.within((*member, from), (word(sup, 1), to))),
                (d::LIST, d::LIST) => self.within((word(sub, 1), from), (word(sup, 1), to)),
                // A cell is written as well as read, so it holds the same
                // type or it is not within.
                (d::CELL, d::CELL) => self.same((word(sub, 1), from), (word(sup, 1), to)),
                (d::FUNCTION, d::FUNCTION) => {
                    let (have, want) = (function(sub), function(sup));
                    // A function that takes less from its caller can
                    // stand in for one that takes more.
                    have.row.iter().all(|ability| want.row.contains(ability))
                        && self.each((want.params, to), (have.params, from))
                        && self.within((have.ret, from), (want.ret, to))
                }
                // Nothing is known of a parameter read outside any scope.
                (d::PARAMETER, _) | (_, d::PARAMETER) => false,
                _ => below == above,
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
