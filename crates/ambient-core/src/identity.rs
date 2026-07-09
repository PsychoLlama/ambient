//! Ability method identity.
//!
//! An ability's identity is its declaration uuid ([`AbilityId::from_uuid`]);
//! a *method's* identity is a [`MethodKey`]: the blake3 hash of the ability
//! uuid, the method's canonical signature, and the content hash of its
//! default implementation. The method **name** is deliberately excluded —
//! renaming a method never changes its key — and the implementation hash is
//! deliberately included, so two same-signature methods in one ability are
//! distinct as long as their bodies differ (a body calling `extern fn
//! stdio_out` hashes apart from one calling `stdio_err`), and changing a
//! method's default behavior re-keys it loudly instead of silently binding
//! old callers to new semantics.
//!
//! Dispatch keys on `(AbilityId, MethodKey)`: a handler arm matches a
//! perform only if both sides derived the same key, which means the same
//! ability uuid, the same signature, and the same default implementation.
//! That is the property remote execution relies on — a function compiled
//! against version N of an ability cannot silently dispatch against a
//! handler compiled for version N+1.

use std::fmt;

use uuid::Uuid;

/// Domain separator for canonical signature hashes.
const SIG_DOMAIN: &[u8] = b"ambient/ability-sig/v1";

/// Domain separator for method keys.
const METHOD_DOMAIN: &[u8] = b"ambient/ability-method/v1";

/// The hash of one method's canonical signature: its parameter types and
/// return type in canonical string form (see the engine's
/// `CanonicalTypeRenderer`), length-prefixed and domain-separated.
///
/// The signature hash is one of the three inputs to a [`MethodKey`]. It is
/// kept as its own value (rather than re-rendering types at every use) so
/// perform sites, handler arms, and the VM's Exception anchor can all
/// derive keys from plain data.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct SignatureHash([u8; 32]);

impl SignatureHash {
    /// Hash a canonical signature: parameter type renderings in order plus
    /// the return type rendering.
    #[must_use]
    pub fn new(params: &[impl AsRef<str>], ret: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SIG_DOMAIN);
        #[allow(clippy::cast_possible_truncation)]
        let count = params.len() as u32;
        hasher.update(&count.to_le_bytes());
        for param in params {
            write_str(&mut hasher, param.as_ref());
        }
        write_str(&mut hasher, ret);
        Self(*hasher.finalize().as_bytes())
    }

    /// Construct from raw hash bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw hash bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SignatureHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignatureHash({})", hex_prefix(&self.0))
    }
}

/// Content-addressed identity of one ability method.
///
/// `derive`d from the ability uuid, the canonical signature, and the
/// default implementation's content hash (`None` only for the abstract
/// `Exception::throw`, whose unhandled behavior is the VM's own
/// uncaught-exception path).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MethodKey([u8; 32]);

impl MethodKey {
    /// Derive a method's identity from its three inputs.
    #[must_use]
    pub fn derive(
        ability_uuid: &Uuid,
        signature: &SignatureHash,
        impl_hash: Option<&[u8; 32]>,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(METHOD_DOMAIN);
        hasher.update(ability_uuid.as_bytes());
        hasher.update(signature.as_bytes());
        match impl_hash {
            Some(hash) => {
                hasher.update(&[1]);
                hasher.update(hash);
            }
            None => {
                hasher.update(&[0]);
            }
        }
        Self(*hasher.finalize().as_bytes())
    }

    /// Construct from raw hash bytes.
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
}

impl fmt::Debug for MethodKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MethodKey({})", self.short_hex())
    }
}

impl fmt::Display for MethodKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Write a length-prefixed string into the hasher.
fn write_str(hasher: &mut blake3::Hasher, s: &str) {
    #[allow(clippy::cast_possible_truncation)]
    let len = s.len() as u32;
    hasher.update(&len.to_le_bytes());
    hasher.update(s.as_bytes());
}

fn hex_prefix(bytes: &[u8; 32]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig() -> SignatureHash {
        SignatureHash::new(&["string"], "unit")
    }

    #[test]
    fn deterministic() {
        let uuid = Uuid::from_u128(7);
        let impl_hash = [3u8; 32];
        let a = MethodKey::derive(&uuid, &sig(), Some(&impl_hash));
        let b = MethodKey::derive(&uuid, &sig(), Some(&impl_hash));
        assert_eq!(a, b);
    }

    #[test]
    fn every_input_changes_the_key() {
        let uuid = Uuid::from_u128(7);
        let impl_hash = [3u8; 32];
        let base = MethodKey::derive(&uuid, &sig(), Some(&impl_hash));

        let other_uuid = MethodKey::derive(&Uuid::from_u128(8), &sig(), Some(&impl_hash));
        assert_ne!(base, other_uuid);

        let other_sig = MethodKey::derive(
            &uuid,
            &SignatureHash::new(&["number"], "unit"),
            Some(&impl_hash),
        );
        assert_ne!(base, other_sig);

        let other_impl = MethodKey::derive(&uuid, &sig(), Some(&[4u8; 32]));
        assert_ne!(base, other_impl);

        let abstract_method = MethodKey::derive(&uuid, &sig(), None);
        assert_ne!(base, abstract_method);
    }

    #[test]
    fn signature_hash_is_length_prefixed() {
        // ["ab", "c"] must not collide with ["a", "bc"].
        let a = SignatureHash::new(&["ab", "c"], "unit");
        let b = SignatureHash::new(&["a", "bc"], "unit");
        assert_ne!(a, b);
    }

    #[test]
    fn ability_id_from_uuid_is_stable_and_distinct() {
        use crate::AbilityId;
        let a = AbilityId::from_uuid(&Uuid::from_u128(1));
        assert_eq!(a, AbilityId::from_uuid(&Uuid::from_u128(1)));
        assert_ne!(a, AbilityId::from_uuid(&Uuid::from_u128(2)));
        // Domain-hashed, not embedded.
        assert_ne!(&a.as_bytes()[..16], Uuid::from_u128(1).as_bytes());
    }
}
