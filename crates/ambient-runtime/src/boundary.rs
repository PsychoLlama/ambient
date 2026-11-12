//! Boundaries: a fault as an error. See `spec/values.md` and
//! `spec/errors.md`.

use crate::object::amb_release;
use crate::record::{amb_record_new, amb_record_set};
use crate::text::{amb_intern, amb_string_new, amb_tag};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The package the kinds of the language's own errors are tags of.
pub const FAULT_PACKAGE: &str = "ambient";

impl Fault {
    /// The kind and message of the error a boundary makes of this, if it
    /// is a fault: an abort and a cancellation pass through boundaries.
    #[must_use]
    pub fn describe(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Self::None | Self::Abort | Self::Cancelled | Self::Exhausted => return None,
            Self::Overflow => ("overflow", "integer arithmetic left the range of int"),
            Self::NoMatch => ("no_match", "no arm of a match was taken"),
            Self::Expired => (
                "expired",
                "an abort handler was performed after its provide had exited",
            ),
            Self::Native => ("native", "native code failed"),
            Self::CleanupAbort => ("cleanup_abort", "a cleanup aborted to a provide outside it"),
        })
    }
}

/// Adds the field `name` to a record under construction, giving up
/// `value`.
pub(crate) unsafe fn set(record: Word, name: &str, value: Word) {
    unsafe {
        let key = amb_intern(name.as_ptr(), name.len() as u64);
        amb_record_set(record, key, value);
        amb_release(value);
    }
}

/// Called by a boundary that its body unwound through. If the unwinding
/// is a fault, ends it and returns the error that describes it, which
/// the caller owns; if not, returns [`UNWIND`] and it goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_contain(context: *mut Context) -> Word {
    unsafe {
        let Some((kind, message)) = Fault::from_code((*context).fault).describe() else {
            return UNWIND;
        };
        (*context).fault = Fault::None as u64;
        let kind = format!("{FAULT_PACKAGE}\0{kind}");
        let error = amb_record_new(4);
        set(error, "kind", amb_tag(kind.as_ptr(), kind.len() as u64));
        let text = amb_string_new(message.as_ptr(), message.len() as u64);
        set(error, "message", text);
        set(error, "cause", NULL);
        set(error, "trace", crate::trace::take(context));
        error
    }
}
