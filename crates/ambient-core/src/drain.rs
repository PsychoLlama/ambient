//! Drain ability identity — the runtime's cooperative-cancellation anchor.
//!
//! `Drain` is declared in Ambient source (`core::system::drain`, in the
//! platform crate) like any other ability; nothing about interruption
//! mechanics lives here. What lives here is only its **identity**: the
//! reserved declaration uuid and the derived [`MethodKey`] of
//! `requested`, which the platform's interruptible natives put inside
//! `VmError::Interrupted` so the VM delivers `Drain::requested!` at the
//! interrupted perform site (see `ref/live-upgrade.md`, "Drain"). Both
//! sides recognize the method by these anchors (the Exception-anchor
//! precedent), never by name.
//!
//! `requested` returns `!` (never) and is **abstract** — no default
//! implementation. Performing it unwinds to the nearest
//! `Drain::requested` arm; with no arm in scope it is an
//! unhandled-ability fault the draining host observes.

use std::sync::OnceLock;

use uuid::Uuid;

use crate::AbilityId;
use crate::identity::{MethodKey, SignatureHash};

/// The Drain ability's reserved declaration uuid (slot `0xD` of the
/// reserved platform ability block `FFFFFFFF-FFFF-FFFF-FFFD-…`). The
/// platform's `drain.ab` carries exactly this uuid; a golden test pins
/// the two together.
pub const DRAIN_UUID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFD_0000_0000_000D);

/// The uuid-derived identity of the Drain ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| AbilityId::from_uuid(&DRAIN_UUID))
}

/// The canonical signature of `requested`: `() -> never`.
#[must_use]
pub fn requested_signature() -> SignatureHash {
    SignatureHash::new(&[] as &[&str], "never")
}

/// The content-addressed identity of `Drain::requested`.
///
/// Derived with no implementation hash (the abstract carve-out): the
/// platform's interrupt deliveries and compiled `Drain::requested` arms
/// both key on exactly this value.
#[must_use]
pub fn requested_method_key() -> MethodKey {
    static KEY: OnceLock<MethodKey> = OnceLock::new();
    *KEY.get_or_init(|| MethodKey::derive(&DRAIN_UUID, &requested_signature(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_uuid_is_the_reserved_slot() {
        assert_eq!(
            DRAIN_UUID.to_string(),
            "ffffffff-ffff-ffff-fffd-00000000000d"
        );
    }

    #[test]
    fn anchors_are_stable() {
        assert_eq!(ability_id(), AbilityId::from_uuid(&DRAIN_UUID));
        assert_eq!(
            requested_method_key(),
            MethodKey::derive(&DRAIN_UUID, &SignatureHash::new(&[] as &[&str], "never"), None)
        );
    }
}
