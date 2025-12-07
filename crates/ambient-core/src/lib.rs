//! Core abilities for the Ambient language.
//!
//! This crate defines the essential abilities that the language depends on,
//! such as `Exception`. These abilities are always available and cannot be
//! disabled, as the language semantics depend on them.
//!
//! Host-provided capabilities like Console, Time, and File operations are
//! defined in `ambient-runtime` instead, as they are environment-specific.

mod descriptor;
pub mod exception;

pub use descriptor::{
    AbilityDescriptor, AbilityProvider, MethodDescriptor, MethodSignature, TypeFactory,
};
pub use exception::{CoreAbilities, EXCEPTION};

/// Ability ID type alias.
pub type AbilityId = u16;

/// Method ID type alias.
pub type MethodId = u16;

/// Reserved ability ID range for core abilities: 0x0000-0x00FF
pub const CORE_ABILITY_RANGE_START: AbilityId = 0x0000;
pub const CORE_ABILITY_RANGE_END: AbilityId = 0x00FF;

/// Reserved ability ID range for runtime abilities: 0x0100-0x0FFF
pub const RUNTIME_ABILITY_RANGE_START: AbilityId = 0x0100;
pub const RUNTIME_ABILITY_RANGE_END: AbilityId = 0x0FFF;

/// Reserved ability ID range for user-defined abilities: 0x1000-0xFFFF
pub const USER_ABILITY_RANGE_START: AbilityId = 0x1000;
