//! Packs: definitions as they travel between programs. See
//! `docs/spec/exchange.md`.
//!
//! A pack holds closed core trees and nothing else: no symbol, no hash of
//! its own contents, no compiled code. Whoever receives one works every
//! hash out again from the trees, so a pack cannot lie about what it
//! holds.

use serde::{Deserialize, Serialize};

use crate::core::{Annotation, Tree};
use crate::deep;
use crate::hash::Hash;

/// What a slot of a packed definition refers to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// Another definition of the pack, by position.
    Inside(usize),
    /// A definition the receiver is expected to have, by hash.
    Outside(Hash),
}

/// A core definition whose slots no longer name anything in a workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub tree: Tree,
    pub slots: Vec<Target>,
    /// Its annotations, which its hash covers as it does its tree.
    pub annotations: Vec<Annotation>,
}

/// A definition and the dependencies its receiver lacks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    pub definitions: Vec<Closed>,
    /// The position of the definition the pack was made for.
    pub entry: usize,
}

/// Why a receiver cannot take a pack in, apart from what it holds being
/// ill-typed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refused {
    /// It refers to a definition its receiver does not have.
    Missing(Hash),
    /// It refers outside itself.
    Malformed,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(hash) => write!(f, "no definition has the hash {hash}"),
            Self::Malformed => write!(f, "a pack refers outside itself"),
        }
    }
}

/// Bytes that are not a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a pack")
    }
}

impl std::error::Error for Malformed {}

impl Pack {
    /// The pack as bytes to send.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        deep(|| crate::hash::encode(self))
    }

    /// The pack `bytes` hold. Nothing about it is trusted yet: see
    /// `Engine::receive`.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not a pack, or refer outside it. A pack
    /// whose trees nest too deeply is not one: see [`crate::core::DEPTH`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Malformed> {
        let pack: Self = deep(|| postcard::from_bytes(bytes)).map_err(|_| Malformed)?;
        let inside = |target: &Target| match target {
            Target::Inside(position) => *position < pack.definitions.len(),
            Target::Outside(_) => true,
        };
        let closed = pack
            .definitions
            .iter()
            .all(|definition| definition.slots.iter().all(inside));
        if closed && pack.entry < pack.definitions.len() {
            Ok(pack)
        } else {
            Err(Malformed)
        }
    }
}
