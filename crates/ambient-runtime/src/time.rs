//! Time ability - for time-related operations.

use ambient_core::AbilityId;

/// Time ability ID.
///
/// This uses the historical ID 0x0003 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0003;

/// Method: get current timestamp in milliseconds.
pub const METHOD_NOW: u16 = 0x0000;

/// Method: wait for a duration in milliseconds.
pub const METHOD_WAIT: u16 = 0x0001;

/// Time ability marker.
pub const TIME: TimeAbility = TimeAbility;

/// Marker type for the Time ability.
#[derive(Clone, Copy)]
pub struct TimeAbility;

impl TimeAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Time";
}
