//! State ability identity — the compiler's fingerprint anchor.
//!
//! `State` is declared in Ambient source (`core::system::state`, in the
//! platform crate) like any other ability; nothing about its cell table
//! lives here. What lives here is only its **identity**: the reserved
//! declaration uuid the compiler keys on to thread migration fingerprints
//! through `State` performs (see `ref/live-upgrade.md`, "Migration").
//!
//! The write-path methods (`init`, `set`, `update`, `init_versioned`)
//! declare trailing `String` fingerprint parameters that user code never
//! supplies: the checker hides them from perform-site arity and records
//! the canonical rendering of the instantiated cell type, and the compiler
//! pushes those renderings as hidden trailing arguments — the same shape
//! as trait-bound dictionaries. Both sides recognize the ability by this
//! uuid (the Exception-anchor precedent), never by name.

use std::sync::OnceLock;

use uuid::Uuid;

use crate::AbilityId;

/// The State ability's reserved declaration uuid (slot `0xC` of the
/// reserved platform ability block `FFFFFFFF-FFFF-FFFF-FFFD-…`). The
/// platform's `state.ab` carries exactly this uuid; a golden test pins
/// the two together.
pub const STATE_UUID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFD_0000_0000_000C);

/// The uuid-derived identity of the State ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| AbilityId::from_uuid(&STATE_UUID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_uuid_is_the_reserved_slot() {
        assert_eq!(
            STATE_UUID.to_string(),
            "ffffffff-ffff-ffff-fffd-00000000000c"
        );
        assert_eq!(ability_id(), AbilityId::from_uuid(&STATE_UUID));
    }
}
