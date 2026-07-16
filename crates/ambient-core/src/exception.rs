//! Exception ability - core error handling.
//!
//! The Exception ability is fundamental to the language's error handling
//! semantics. It provides the `throw` method for raising errors that can
//! be caught by handlers.
//!
//! Exception is declared in Ambient source (`core::exception`, re-exported
//! from the prelude) like any other ability; it is not an engine builtin.
//! What lives here is only its *identity*: the reserved declaration uuid,
//! the uuid-derived [`AbilityId`] the VM's Rust-level raise channel keys
//! on, and the [`MethodKey`] of `throw`. The in-language declaration
//! reproduces these exactly, so the two never drift.
//!
//! Like every ability method, `throw` carries a default implementation —
//! the behavior of an unhandled perform. Its body delivers the thrown
//! value to the host through the module-private `extern fn uncaught`
//! (`core_lib/exception.ab`), so an unhandled `throw` surfaces as an
//! uncaught exception with no VM special-casing. Because a method's
//! identity folds in its default implementation's content hash, that
//! compiled body's hash is pinned here ([`THROW_IMPL_HASH`]) — the one
//! anchor the VM's raise channel (`Vm::raise_exception`, how a native's
//! `Err(VmError::Exception)` finds in-language handlers) needs without
//! compiling core itself. A golden test pins it against the real compiled
//! core, so an edit to the body (or to codegen) fails loudly instead of
//! silently splitting the raise channel from compiled handlers.

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

/// The canonical signature of `throw`: `<E: Error>(E) -> never`.
///
/// The generic parameter `E` renders as `var0` (the first type variable, by
/// first occurrence — the checker substitutes the type parameter to an
/// inference variable before rendering), and its `Error` bound enters the
/// canonical signature as the `bound:0:Error` pseudo-parameter after the real
/// ones (index `0` into the method's type parameters, *spelled* name by the
/// same convention as `named:Duration`). This must reproduce byte-for-byte
/// what the checker derives for `core::exception`'s declaration
/// (`ability_id_infer`'s seeded renderer) — a golden test pins the two.
#[must_use]
pub fn throw_signature() -> SignatureHash {
    SignatureHash::new(&["var0", "bound:0:Error"], "never")
}

/// Content hash of `throw`'s compiled default implementation — the
/// `core::exception` body that delivers an unhandled throw to the host
/// (`fn throw<E: Error>(error: E): ! { uncaught(error) }`).
///
/// Pinned here because the implementation hash is a [`MethodKey`] input
/// and the VM's raise channel must derive the same `throw` key as compiled
/// perform sites and handler arms — without compiling core. A golden test
/// re-compiles core and compares byte-for-byte; when it fails (the body or
/// codegen changed), update this literal to the hash it reports.
pub const THROW_IMPL_HASH: [u8; 32] = [
    0xb8, 0x6f, 0x95, 0x4e, 0x97, 0xde, 0x1b, 0xda, 0xb9, 0xcf, 0x2c, 0x36, 0x29, 0xfe, 0x3c, 0x67,
    0x0f, 0x80, 0xda, 0x56, 0xa1, 0xe4, 0x3f, 0x45, 0x6d, 0xe0, 0x76, 0xbf, 0x13, 0x8d, 0xa5, 0xa6,
];

/// The content-addressed identity of `Exception::throw`.
///
/// Derived from the reserved uuid, the canonical signature, and the pinned
/// default-implementation hash: the VM's `raise_exception` path and
/// compiled `Exception::throw!` sites both key on exactly this value.
#[must_use]
pub fn throw_method_key() -> MethodKey {
    static KEY: OnceLock<MethodKey> = OnceLock::new();
    *KEY.get_or_init(|| {
        MethodKey::derive(&EXCEPTION_UUID, &throw_signature(), Some(&THROW_IMPL_HASH))
    })
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
                &SignatureHash::new(&["var0", "bound:0:Error"], "never"),
                Some(&THROW_IMPL_HASH)
            )
        );
    }
}
