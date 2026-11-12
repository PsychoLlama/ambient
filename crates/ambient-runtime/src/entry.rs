//! The entry points: what compiled code may call, and in what order they are tabled.

use crate::{
    amb_closure_new, amb_closure_set, amb_equal, amb_fault, amb_float_add, amb_float_less,
    amb_float_multiply, amb_float_new, amb_float_subtract, amb_int_add, amb_int_less,
    amb_int_multiply, amb_int_new, amb_int_subtract, amb_is, amb_record_get, amb_record_has,
    amb_record_is, amb_record_new, amb_record_set, amb_release, amb_retain, amb_sequence_get,
    amb_sequence_is, amb_sequence_new, amb_sequence_set,
};

/// The entry points, in table order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Entry {
    Retain,
    Release,
    IntAdd,
    IntSubtract,
    IntMultiply,
    IntLess,
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatLess,
    Equal,
    IntNew,
    FloatNew,
    Fault,
    SequenceNew,
    SequenceSet,
    SequenceGet,
    SequenceIs,
    RecordNew,
    RecordSet,
    RecordGet,
    RecordHas,
    RecordIs,
    ClosureNew,
    ClosureSet,
    Is,
}

impl Entry {
    pub const ALL: [Self; 26] = [
        Self::Retain,
        Self::Release,
        Self::IntAdd,
        Self::IntSubtract,
        Self::IntMultiply,
        Self::IntLess,
        Self::FloatAdd,
        Self::FloatSubtract,
        Self::FloatMultiply,
        Self::FloatLess,
        Self::Equal,
        Self::IntNew,
        Self::FloatNew,
        Self::Fault,
        Self::SequenceNew,
        Self::SequenceSet,
        Self::SequenceGet,
        Self::SequenceIs,
        Self::RecordNew,
        Self::RecordSet,
        Self::RecordGet,
        Self::RecordHas,
        Self::RecordIs,
        Self::ClosureNew,
        Self::ClosureSet,
        Self::Is,
    ];

    /// The name the entry point is exported under.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Retain => "amb_retain",
            Self::Release => "amb_release",
            Self::IntAdd => "amb_int_add",
            Self::IntSubtract => "amb_int_subtract",
            Self::IntMultiply => "amb_int_multiply",
            Self::IntLess => "amb_int_less",
            Self::FloatAdd => "amb_float_add",
            Self::FloatSubtract => "amb_float_subtract",
            Self::FloatMultiply => "amb_float_multiply",
            Self::FloatLess => "amb_float_less",
            Self::Equal => "amb_equal",
            Self::IntNew => "amb_int_new",
            Self::FloatNew => "amb_float_new",
            Self::Fault => "amb_fault",
            Self::SequenceNew => "amb_sequence_new",
            Self::SequenceSet => "amb_sequence_set",
            Self::SequenceGet => "amb_sequence_get",
            Self::SequenceIs => "amb_sequence_is",
            Self::RecordNew => "amb_record_new",
            Self::RecordSet => "amb_record_set",
            Self::RecordGet => "amb_record_get",
            Self::RecordHas => "amb_record_has",
            Self::RecordIs => "amb_record_is",
            Self::ClosureNew => "amb_closure_new",
            Self::ClosureSet => "amb_closure_set",
            Self::Is => "amb_is",
        }
    }

    /// Whether the entry point takes the context as its first argument.
    #[must_use]
    pub fn takes_context(self) -> bool {
        matches!(
            self,
            Self::IntAdd | Self::IntSubtract | Self::IntMultiply | Self::Fault
        )
    }

    /// How many words it takes, besides the context.
    #[must_use]
    pub fn arity(self) -> usize {
        match self {
            Self::Retain
            | Self::Release
            | Self::IntNew
            | Self::FloatNew
            | Self::Fault
            | Self::RecordNew
            | Self::RecordIs => 1,
            Self::SequenceSet | Self::SequenceIs | Self::RecordSet | Self::ClosureSet => 3,
            _ => 2,
        }
    }

    /// Whether it returns a word. The others return nothing.
    #[must_use]
    pub fn returns(self) -> bool {
        !matches!(
            self,
            Self::Retain | Self::Release | Self::SequenceSet | Self::RecordSet | Self::ClosureSet
        )
    }

    /// The native address of the entry point.
    #[must_use]
    pub fn address(self) -> usize {
        match self {
            Self::Retain => amb_retain as *const () as usize,
            Self::Release => amb_release as *const () as usize,
            Self::IntAdd => amb_int_add as *const () as usize,
            Self::IntSubtract => amb_int_subtract as *const () as usize,
            Self::IntMultiply => amb_int_multiply as *const () as usize,
            Self::IntLess => amb_int_less as *const () as usize,
            Self::FloatAdd => amb_float_add as *const () as usize,
            Self::FloatSubtract => amb_float_subtract as *const () as usize,
            Self::FloatMultiply => amb_float_multiply as *const () as usize,
            Self::FloatLess => amb_float_less as *const () as usize,
            Self::Equal => amb_equal as *const () as usize,
            Self::IntNew => amb_int_new as *const () as usize,
            Self::FloatNew => amb_float_new as *const () as usize,
            Self::Fault => amb_fault as *const () as usize,
            Self::SequenceNew => amb_sequence_new as *const () as usize,
            Self::SequenceSet => amb_sequence_set as *const () as usize,
            Self::SequenceGet => amb_sequence_get as *const () as usize,
            Self::SequenceIs => amb_sequence_is as *const () as usize,
            Self::RecordNew => amb_record_new as *const () as usize,
            Self::RecordSet => amb_record_set as *const () as usize,
            Self::RecordGet => amb_record_get as *const () as usize,
            Self::RecordHas => amb_record_has as *const () as usize,
            Self::RecordIs => amb_record_is as *const () as usize,
            Self::ClosureNew => amb_closure_new as *const () as usize,
            Self::ClosureSet => amb_closure_set as *const () as usize,
            Self::Is => amb_is as *const () as usize,
        }
    }
}
