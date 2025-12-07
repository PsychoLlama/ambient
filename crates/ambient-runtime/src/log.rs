//! Log ability - for structured logging with levels.

use ambient_core::AbilityId;

/// Log ability ID.
///
/// This uses the historical ID 0x0006 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0006;

/// Method: log a debug message.
pub const METHOD_DEBUG: u16 = 0x0000;

/// Method: log an info message.
pub const METHOD_INFO: u16 = 0x0001;

/// Method: log a warning message.
pub const METHOD_WARN: u16 = 0x0002;

/// Method: log an error message.
pub const METHOD_ERROR: u16 = 0x0003;

/// Log ability marker.
pub const LOG: LogAbility = LogAbility;

/// Marker type for the Log ability.
#[derive(Clone, Copy)]
pub struct LogAbility;

impl LogAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Log";
}
