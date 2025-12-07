//! Async ability - for concurrent execution of abilities.

use ambient_core::AbilityId;

/// Async ability ID.
///
/// This uses the historical ID 0x0005 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0005;

/// Method: wait for all operations to complete.
/// Takes a list of suspended abilities, returns a list of results.
pub const METHOD_ALL: u16 = 0x0000;

/// Method: wait for first operation to complete, cancel others.
/// Takes a list of suspended abilities, returns the first result.
pub const METHOD_RACE: u16 = 0x0001;

/// Async ability marker.
pub const ASYNC: AsyncAbility = AsyncAbility;

/// Marker type for the Async ability.
#[derive(Clone, Copy)]
pub struct AsyncAbility;

impl AsyncAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Async";
}
