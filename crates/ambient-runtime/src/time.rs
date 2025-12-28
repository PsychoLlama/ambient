//! Time ability - for time-related operations.

use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value};
use ambient_core::{
    AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature, TypeFactory,
};

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

// ═══════════════════════════════════════════════════════════════════════════
// Time RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Time ability implementation combining type info and handlers.
#[derive(Default)]
pub struct TimeRuntimeAbility;

impl TimeRuntimeAbility {
    /// Create a new Time ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for TimeRuntimeAbility {
    fn name(&self) -> &'static str {
        "Time"
    }

    fn ability_id(&self) -> AbilityId {
        ABILITY_ID
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ABILITY_ID,
            name: "Time",
            methods: Box::leak(Box::new([
                MethodDescriptor {
                    id: METHOD_NOW,
                    name: "now",
                    signature: MethodSignature {
                        param_count: 0,
                        param_types: |_f| vec![],
                        return_type: |f| f.number(),
                    },
                },
                MethodDescriptor {
                    id: METHOD_WAIT,
                    name: "wait",
                    signature: MethodSignature {
                        param_count: 1,
                        param_types: |f| vec![f.number()],
                        return_type: |f| f.unit(),
                    },
                },
            ])),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        let now = Box::new(|_ability: &SuspendedAbility| {
            use std::time::{SystemTime, UNIX_EPOCH};
            #[allow(clippy::cast_precision_loss)]
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0);
            Ok(Value::Number(now))
        }) as HostHandler;

        let wait = Box::new(|ability: &SuspendedAbility| {
            if let Some(Value::Number(ms)) = ability.args.first() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms_u64 = if *ms < 0.0 { 0 } else { *ms as u64 };
                let duration = std::time::Duration::from_millis(ms_u64);
                std::thread::sleep(duration);
            }
            Ok(Value::Unit)
        }) as HostHandler;

        vec![(METHOD_NOW, now), (METHOD_WAIT, wait)]
    }
}
