//! Console ability - for printing to stdout/stderr.

use ambient_core::AbilityId;

/// Console ability ID.
///
/// This uses the historical ID 0x0001 for backward compatibility.
pub const ABILITY_ID: AbilityId = 0x0001;

/// Method: print a message to stdout.
pub const METHOD_PRINT: u16 = 0x0000;

/// Method: print a message to stderr.
pub const METHOD_EPRINT: u16 = 0x0001;

/// Method: print with newline.
pub const METHOD_PRINTLN: u16 = 0x0002;

/// Console ability marker.
pub const CONSOLE: ConsoleAbility = ConsoleAbility;

/// Marker type for the Console ability.
#[derive(Clone, Copy)]
pub struct ConsoleAbility;

impl ConsoleAbility {
    /// Ability ID.
    pub const ABILITY_ID: AbilityId = ABILITY_ID;

    /// Ability name.
    pub const NAME: &'static str = "Console";
}
