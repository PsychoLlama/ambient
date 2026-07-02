//! Core abilities for the Ambient language.
//!
//! This crate defines the essential abilities that the language depends on,
//! such as `Exception`. These abilities are always available and cannot be
//! disabled, as the language semantics depend on them.
//!
//! Host-provided capabilities like Console, Time, and File operations are
//! defined in `ambient-runtime` instead, as they are environment-specific.

mod canonical;
mod descriptor;
pub mod exception;

use std::fmt;

pub use canonical::{
    hash_interface, hash_interface_raw, CanonicalType, CanonicalTypeFactory, RawMethod,
};
pub use descriptor::{
    AbilityDescriptor, AbilityProvider, MethodDescriptor, MethodSignature, TypeFactory,
};
pub use exception::{CoreAbilities, EXCEPTION};

/// Content-addressed ability identity.
///
/// An ability is identified by the blake3 hash of its canonical interface:
/// its name plus the ordered list of method names and canonicalized
/// signatures (see [`hash_interface`]). Two engines that compute the same
/// `AbilityId` agree on exactly what the ability's methods mean, which is
/// what allows handlers and suspended ability calls to travel between
/// engine instances.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct AbilityId([u8; 32]);

impl AbilityId {
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

/// Method ID type alias.
///
/// Methods are identified by their declaration index within the ability.
/// Because the ability's `AbilityId` commits to the full ordered method
/// list, `(AbilityId, MethodId)` is globally unambiguous.
pub type MethodId = u16;

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
