//! Core abilities for the Ambient language.
//!
//! This crate defines the essential abilities that the language depends on,
//! such as `Exception`. These abilities are always available and cannot be
//! disabled, as the language semantics depend on them.
//!
//! Host-provided capabilities like Stdio, Time, and File operations are
//! defined in `ambient-platform` instead, as they are environment-specific.

pub mod drain;
pub mod exception;
mod identity;
pub mod state;

use std::fmt;

pub use identity::{MethodKey, SignatureHash};

/// Nominal ability identity, derived from the declaration uuid.
///
/// An ability *is* its `unique(<uuid>)` prefix ([`Self::from_uuid`]): two
/// engines that see the same uuid agree on which ability is meant, and
/// per-method agreement (signatures and behavior) is carried separately
/// by [`MethodKey`]. This is what allows handlers and suspended ability
/// calls to travel between engine instances.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct AbilityId([u8; 32]);

impl AbilityId {
    /// Derive an ability's identity from its declaration uuid.
    ///
    /// Abilities are nominal: the `unique(<uuid>)` prefix *is* the
    /// identity, so renaming an ability, renaming its methods, or moving
    /// the declaration to another module never changes its `AbilityId`,
    /// and two same-shaped abilities with different uuids never collide.
    /// The uuid is domain-hashed (rather than embedded) so an `AbilityId`
    /// is the same shape as every other 32-byte identity and is never
    /// parsed back into its inputs.
    #[must_use]
    pub fn from_uuid(uuid: &uuid::Uuid) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ambient/ability-id/v2");
        hasher.update(uuid.as_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    /// Construct an ability ID from raw hash bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw hash bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Full lowercase hex encoding (64 characters).
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Abbreviated hex for human-facing output (first 12 characters).
    #[must_use]
    pub fn short_hex(&self) -> String {
        let mut hex = self.to_hex();
        hex.truncate(12);
        hex
    }

    /// Parse a full 64-character hex encoding.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            #[allow(clippy::cast_possible_truncation)]
            {
                bytes[i] = ((hi << 4) | lo) as u8;
            }
        }
        Some(Self(bytes))
    }
}

impl fmt::Debug for AbilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AbilityId({})", self.short_hex())
    }
}

impl fmt::Display for AbilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod ability_id_tests {
    use super::AbilityId;

    #[test]
    fn hex_roundtrip() {
        let id = AbilityId::from_bytes([0xab; 32]);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(AbilityId::from_hex(&hex), Some(id));
    }

    #[test]
    fn from_hex_rejects_bad_input() {
        assert_eq!(AbilityId::from_hex("abc"), None);
        assert_eq!(AbilityId::from_hex(&"zz".repeat(32)), None);
    }
}
