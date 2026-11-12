//! Content hashes and the helpers that feed them.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A 256-bit content hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    /// The raw bytes of the hash.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#")?;
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Builds a hash from a sequence of parts.
///
/// Every hasher starts from a domain, so hashes made for different purposes
/// never collide. Variable-length parts are length-prefixed, so two different
/// sequences of parts never hash the same.
pub struct Hasher(blake3::Hasher);

impl Hasher {
    #[must_use]
    pub fn new(domain: &str) -> Self {
        let mut hasher = Self(blake3::Hasher::new());
        hasher.bytes(domain.as_bytes());
        hasher
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.u64(bytes.len() as u64);
        self.0.update(bytes);
        self
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.0.update(&value.to_le_bytes());
        self
    }

    pub fn hash(&mut self, hash: &Hash) -> &mut Self {
        self.0.update(&hash.0);
        self
    }

    #[must_use]
    pub fn finish(&self) -> Hash {
        Hash(*self.0.finalize().as_bytes())
    }
}

/// Serializes one of this crate's own values for a store entry or a digest.
///
/// This is not the canonical encoding of a core tree; see
/// [`crate::core::Tree::encode`] for that.
pub(crate) fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    match postcard::to_allocvec(value) {
        Ok(bytes) => bytes,
        // Writing to memory cannot fail, and every type passed here derives
        // a serializer that postcard supports.
        Err(error) => unreachable!("in-memory serialization failed: {error}"),
    }
}

/// A digest of one of this crate's own values, for use in a stage key.
pub(crate) fn digest<T: Serialize>(domain: &str, value: &T) -> Hash {
    Hasher::new(domain).bytes(&encode(value)).finish()
}
