//! The native code a running program reaches, by the index compiled
//! code names each extern by. See "Linking" in `docs/spec/externs.md`.

use std::ops::Range;

use super::natives::Offered;
use crate::parts::Native;

/// The native code for every extern a program has loaded, in the order
/// the loads numbered them.
///
/// Compiled code holds an index for as long as it is loaded, so an
/// index is never given to anything else meanwhile: what a load added
/// is emptied when the load is dropped, and the list shortens only from
/// its end.
#[derive(Default)]
pub struct Externs(Vec<Option<(Native, Offered)>>);

impl Externs {
    /// The index the next extern added will have.
    #[must_use]
    pub fn next(&self) -> usize {
        self.0.len()
    }

    /// Adds the native code of one load, and says which indices it has.
    pub fn add(&mut self, externs: &[Native], offered: Vec<Offered>) -> Range<usize> {
        let start = self.next();
        let named = externs.iter().cloned().zip(offered);
        self.0.extend(named.map(Some));
        start..self.next()
    }

    /// The extern at an index, and the native code for it.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&(Native, Offered)> {
        self.0.get(index)?.as_ref()
    }

    /// Drops the native code of one load.
    pub fn remove(&mut self, added: Range<usize>) {
        for index in added {
            if let Some(slot) = self.0.get_mut(index) {
                *slot = None;
            }
        }
        while self.0.last().is_some_and(Option::is_none) {
            self.0.pop();
        }
    }
}
