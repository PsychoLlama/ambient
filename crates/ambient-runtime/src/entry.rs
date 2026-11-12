//! The entry points: what compiled code may call, and in what order they
//! are tabled.

/// Declares every entry point once: its name here, the function behind
/// it, whether it takes the context first, how many words it takes
/// besides, and whether it returns one.
macro_rules! entries {
    ($($name:ident = $function:ident, $context:ident, $arity:literal, $returns:ident;)*) => {
        /// The entry points, in table order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum Entry {
            $($name,)*
        }

        impl Entry {
            pub const ALL: &[Self] = &[$(Self::$name,)*];

            /// The name the entry point is exported under.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($function),)*
                }
            }

            /// Whether the entry point takes the context as its first
            /// argument.
            #[must_use]
            pub fn takes_context(self) -> bool {
                match self {
                    $(Self::$name => entries!(@flag $context),)*
                }
            }

            /// How many words it takes, besides the context.
            #[must_use]
            pub fn arity(self) -> usize {
                match self {
                    $(Self::$name => $arity,)*
                }
            }

            /// Whether it returns a word. The others return nothing.
            #[must_use]
            pub fn returns(self) -> bool {
                match self {
                    $(Self::$name => entries!(@flag $returns),)*
                }
            }

            /// The native address of the entry point.
            #[must_use]
            pub fn address(self) -> usize {
                match self {
                    $(Self::$name => crate::$function as *const () as usize,)*
                }
            }
        }
    };
    (@flag context) => { true };
    (@flag word) => { true };
    (@flag plain) => { false };
    (@flag nothing) => { false };
}

entries! {
    Retain = amb_retain, plain, 1, nothing;
    Release = amb_release, plain, 1, nothing;
    IntAdd = amb_int_add, context, 2, word;
    IntSubtract = amb_int_subtract, context, 2, word;
    IntMultiply = amb_int_multiply, context, 2, word;
    IntLess = amb_int_less, plain, 2, word;
    FloatAdd = amb_float_add, plain, 2, word;
    FloatSubtract = amb_float_subtract, plain, 2, word;
    FloatMultiply = amb_float_multiply, plain, 2, word;
    FloatLess = amb_float_less, plain, 2, word;
    Equal = amb_equal, plain, 2, word;
    IntNew = amb_int_new, plain, 1, word;
    FloatNew = amb_float_new, plain, 1, word;
    Fault = amb_fault, context, 1, word;
    SequenceNew = amb_sequence_new, plain, 2, word;
    SequenceSet = amb_sequence_set, plain, 3, nothing;
    SequenceGet = amb_sequence_get, plain, 2, word;
    SequenceIs = amb_sequence_is, plain, 3, word;
    RecordNew = amb_record_new, plain, 1, word;
    RecordSet = amb_record_set, plain, 3, nothing;
    RecordGet = amb_record_get, plain, 2, word;
    RecordHas = amb_record_has, plain, 2, word;
    RecordIs = amb_record_is, plain, 1, word;
    ClosureNew = amb_closure_new, plain, 2, word;
    ClosureSet = amb_closure_set, plain, 3, nothing;
    Is = amb_is, plain, 3, word;
    Trace = amb_trace, context, 2, word;
    RecordMerge = amb_record_merge, plain, 2, word;
    TokenNew = amb_token_new, plain, 0, word;
    TokenExpire = amb_token_expire, plain, 1, word;
    Abort = amb_abort, context, 2, word;
    Caught = amb_caught, context, 1, word;
    Native = amb_native, context, 3, word;
    FramePush = amb_frame_push, context, 3, nothing;
    FrameSave = amb_frame_save, context, 1, nothing;
    FramePoint = amb_frame_point, context, 0, word;
    FrameLoad = amb_frame_load, context, 1, word;
    FrameDrop = amb_frame_drop, context, 0, nothing;
    All = amb_all, context, 1, word;
    First = amb_first, context, 1, word;
    GroupNew = amb_group_new, context, 0, word;
    Spawn = amb_spawn, context, 2, word;
    GroupClose = amb_group_close, context, 2, word;
    GroupEnd = amb_group_end, context, 2, word;
    BudgetEnter = amb_budget_enter, context, 2, word;
    BudgetLeave = amb_budget_leave, context, 1, word;
    BudgetCaught = amb_budget_caught, context, 1, word;
    BudgetCurrent = amb_budget_current, context, 0, word;
    BudgetSwitch = amb_budget_switch, context, 1, word;
    BudgetBack = amb_budget_back, context, 2, word;
    Charge = amb_charge, context, 2, word;
    ChargeSize = amb_charge_size, context, 1, word;
    Contain = amb_contain, context, 0, word;
    Hold = amb_hold, context, 0, word;
    Rethrow = amb_rethrow, context, 1, word;
    CleanupEnter = amb_cleanup_enter, context, 0, word;
    CleanupLeave = amb_cleanup_leave, context, 0, word;
    CleanupFailed = amb_cleanup_failed, context, 0, word;
    CellNew = amb_cell_new, plain, 3, word;
    CellRead = amb_cell_read, plain, 1, word;
    CellReplace = amb_cell_replace, context, 3, word;
    CellWait = amb_cell_wait, context, 2, word;
    Capture = amb_capture, context, 0, word;
    ScopeNew = amb_scope_new, plain, 1, word;
    ScopeSet = amb_scope_set, plain, 4, nothing;
}
