//! What resolving a definition gives, and how a node of it is found in
//! the text.

use serde::{Deserialize, Serialize};

use crate::hash::Hash;
use crate::surface::{Definition, Reference};

#[derive(Debug, Serialize, Deserialize)]
pub struct Resolved {
    pub definition: Definition<Reference>,
    /// A digest of `definition`, kept so no later stage re-derives it.
    pub digest: Hash,
    /// The nodes of the body as written that are no node of the
    /// resolved one, each by its number there, in order: what a call
    /// of an intrinsic leaves out. See [`Self::written`].
    pub skipped: Vec<u32>,
    /// The nodes of the resolved body that nothing wrote, each by its
    /// number there, in order: what a call of an intrinsic adds.
    pub added: Vec<u32>,
    /// The nodes of the resolved body an error is not reported at, each
    /// with the node it is reported at instead: a cast that holds what
    /// it was given, and the binding its call became.
    pub stood: Vec<(u32, u32)>,
}

impl Resolved {
    /// The node of the body as written that the node numbered
    /// `resolved` of the resolved body was made from.
    #[must_use]
    pub fn written(&self, resolved: usize) -> usize {
        let number = |node: &u32| usize::try_from(*node).unwrap_or(usize::MAX);
        let stood = self.stood.iter().find(|(from, _)| number(from) == resolved);
        let resolved = stood.map_or(resolved, |(_, to)| number(to));
        // A node nothing wrote is where the node before it is.
        let added = self.added.iter().map(number);
        let mut written = resolved - added.filter(|added| *added <= resolved).count();
        for skipped in self.skipped.iter().map(number) {
            if skipped > written {
                break;
            }
            written += 1;
        }
        written
    }
}
