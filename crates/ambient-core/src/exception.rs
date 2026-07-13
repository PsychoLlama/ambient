//! Exception ability - core error handling.
//!
//! The Exception ability is fundamental to the language's error handling
//! semantics. It provides the `throw` method for raising errors that can
//! be caught by handlers.
//!
//! Exception is declared in Ambient source (`core::exception`, re-exported
//! from the prelude) like any other ability; it is not an engine builtin.
//! What lives here is only its *identity*: the reserved declaration uuid,
//! the uuid-derived [`AbilityId`] the VM's throw/unwind path keys on, and
//! the [`MethodKey`] of `throw`. The in-language declaration reproduces
//! these exactly, so the two never drift.
//!
//! `throw` is the language's one **abstract** ability method — a signature
//! with no default implementation. Every other ability method carries a
//! body (the behavior of an unhandled perform); an unhandled `throw` is
//! the VM's own uncaught-exception path, which no in-language body could
//! express (`throw` returns `!`).

use std::sync::OnceLock;

use uuid::Uuid;

use crate::AbilityId;
use crate::identity::{MethodKey, SignatureHash};

/// The Exception ability's reserved declaration uuid — the first slot of
/// the reserved ability-identity block (`FFFFFFFF-FFFF-FFFF-FFFD-…`).
/// `core_lib/exception.ab` carries exactly this uuid; a golden test pins
/// the two together.
pub const EXCEPTION_UUID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFD_0000_0000_0001);

/// The uuid-derived identity of the Exception ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| AbilityId::from_uuid(&EXCEPTION_UUID))
}

/// The canonical signature of `throw`: `<E: Show>(E) -> never`.
///
/// The generic parameter `E` renders as `var0` (the first type variable, by
/// first occurrence — the checker substitutes the type parameter to an
/// inference variable before rendering), and its `Show` bound enters the
/// canonical signature as the `bound:0:Show` pseudo-parameter after the real
/// ones (index `0` into the method's type parameters, *spelled* name by the
/// same convention as `named:Duration`). This must reproduce byte-for-byte
/// what the checker derives for `core::exception`'s declaration
/// (`ability_id_infer`'s seeded renderer) — a golden test pins the two.
#[must_use]
pub fn throw_signature() -> SignatureHash {
    SignatureHash::new(&["var0", "bound:0:Show"], "never")
}

/// The content-addressed identity of `Exception::throw`.
///
/// Derived with no implementation hash (the abstract carve-out): the VM's
/// `raise_exception` path and compiled `Exception::throw!` sites both key
/// on exactly this value.
#[must_use]
pub fn throw_method_key() -> MethodKey {
    static KEY: OnceLock<MethodKey> = OnceLock::new();
    *KEY.get_or_init(|| MethodKey::derive(&EXCEPTION_UUID, &throw_signature(), None))
}

/// Ability name.
pub const NAME: &str = "Exception";

/// Method name for throw.
pub const METHOD_THROW_NAME: &str = "throw";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_uuid_is_the_reserved_slot() {
        assert_eq!(
            EXCEPTION_UUID.to_string(),
            "ffffffff-ffff-ffff-fffd-000000000001"
        );
    }

    #[test]
    fn anchors_are_stable() {
        assert_eq!(ability_id(), AbilityId::from_uuid(&EXCEPTION_UUID));
        assert_eq!(
            throw_method_key(),
            MethodKey::derive(
                &EXCEPTION_UUID,
                &SignatureHash::new(&["var0", "bound:0:Show"], "never"),
                None
            )
        );
    }
}
