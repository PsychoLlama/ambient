//! Type arguments at run time: what a type parameter stands for where a
//! node that mentions one is read. See `spec/backends.md`.

use crate::is::descriptor as d;
use crate::object::{KIND_SCOPE, PINNED, amb_retain, kind, object, words};
use crate::relate::Types;
use crate::{NULL, Word};

/// Where the type parameters a node mentions are found.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Env {
    /// Nowhere: the node mentions none.
    None,
    /// In the arguments a generic type is applied to: a [`Scope`], by
    /// position among those a test has entered.
    Entered(usize),
    /// In the type arguments a generic function was made at: an object,
    /// which holds for each a node and what that node is read in.
    Given(Word),
}

impl Env {
    /// What a function made at `scope`, its type arguments or null, reads
    /// its types in.
    pub(crate) fn given(scope: Word) -> Self {
        if scope == NULL {
            Self::None
        } else {
            Self::Given(scope)
        }
    }
}

/// A node, and what it is read in.
pub(crate) type At = (u64, Env);

/// The arguments a generic type definition is being read at.
#[derive(Clone, Copy)]
pub(crate) struct Scope {
    pub(crate) nodes: &'static [u64],
    /// What the arguments themselves are read in.
    pub(crate) outer: Env,
}

/// What the parameter at `position` stands for in `env`, if anything
/// there says.
pub(crate) unsafe fn argument(scopes: &[Scope], env: Env, position: usize) -> Option<At> {
    match env {
        Env::None => None,
        Env::Entered(scope) => {
            let Scope { nodes, outer } = scopes[scope];
            Some((nodes[position], outer))
        }
        Env::Given(scope) => unsafe {
            let entries = words(scope);
            Some((entries[position * 2], Env::given(entries[position * 2 + 1])))
        },
    }
}

/// Writes the header of type arguments that are never reclaimed into
/// `words`: `count` of them, each a node and then null, follow it.
pub fn pinned_scope(words: &mut [u64], count: usize) {
    words[0] = u64::from(PINNED) | (u64::from(KIND_SCOPE) << 32);
    words[1] = count as u64 * 16;
}

/// Room for `count` type arguments. Fill it with [`amb_scope_set`] before
/// anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_scope_new(count: u64) -> Word {
    unsafe {
        let word = object(KIND_SCOPE, count as usize * 16);
        words(word).fill(NULL);
        word
    }
}

/// Makes type argument `index` the type `node` describes, read in
/// `within`: the type arguments of the function this runs in, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_scope_set(scope: Word, index: u64, node: u64, within: Word) {
    unsafe {
        let (mut node, mut within) = (node, within);
        // A parameter is passed on as what it stands for, so a chain of
        // generic calls does not keep every caller's arguments.
        if within != NULL && (node as usize as *const u64).read() == d::PARAMETER {
            let position = (node as usize as *const u64).add(1).read() as usize;
            let entries = words(within);
            (node, within) = (entries[position * 2], entries[position * 2 + 1]);
        }
        amb_retain(within);
        let entries = words(scope);
        entries[index as usize * 2] = node;
        entries[index as usize * 2 + 1] = within;
    }
}

/// The type arguments a closure's function was made at, or null: they
/// follow whatever else it captured.
pub(crate) unsafe fn of_closure(closure: Word) -> Word {
    unsafe {
        match words(closure) {
            [_, .., last] if kind(*last) == KIND_SCOPE => *last,
            _ => NULL,
        }
    }
}

/// Whether two sets of type arguments are the same types.
pub(crate) unsafe fn same(left: Word, right: Word) -> bool {
    unsafe {
        let (left, right) = (words(left), words(right));
        let at = |entry: &[u64]| (entry[0], Env::given(entry[1]));
        left.len() == right.len()
            && left
                .chunks(2)
                .zip(right.chunks(2))
                .all(|(l, r)| Types::new(&[]).same(at(l), at(r)))
    }
}
