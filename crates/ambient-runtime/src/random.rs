//! Random ability - for random number generation.

use ambient_core::AbilityId;

/// Random ability ID.
///
/// This uses the historical ID 0x0004 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0004;

/// Method: get a random number between 0.0 and 1.0.
pub const METHOD_SEED: u16 = 0x0000;

/// Method: get a random number in a range.
pub const METHOD_IN_RANGE: u16 = 0x0001;

/// Random ability marker.
pub const RANDOM: RandomAbility = RandomAbility;

/// Marker type for the Random ability.
#[derive(Clone, Copy)]
pub struct RandomAbility;

impl RandomAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Random";
}
