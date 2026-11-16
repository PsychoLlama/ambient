//! Traces: where a fault was, and the calls it unwound through. See
//! `spec/errors.md`.
//!
//! A trace is gathered as a fault unwinds, or an abort whose handler
//! takes one. Compiled code that either leaves calls [`amb_trace`] on
//! the way out, so the cost is paid only by those. The frames gathered so far are kept in the context as a chain,
//! outermost first: each is a tuple of a definition, a position, the
//! frame inside it, and how long the chain is.

use crate::boundary::set;
use crate::number::amb_int_new;
use crate::object::{amb_release, int, words};
use crate::record::amb_record_new;
use crate::sequence::{amb_sequence_new, amb_sequence_set};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The most frames a trace keeps: those nearest where it begins.
pub const TRACE_LIMIT: usize = 64;

/// Notes that the unwinding under way left `position` of `definition`, a
/// string, if it is one that gathers a trace. Always returns [`UNWIND`]:
/// it goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_trace(context: *mut Context, definition: Word, position: u64) -> Word {
    unsafe {
        let fault = Fault::from_code((*context).fault).describe().is_some();
        if !fault && !crate::abort::traced(context) {
            return UNWIND;
        }
        let inner = (*context).trace;
        let length = if inner == 0 { 0 } else { int(words(inner)[3]) };
        if length >= TRACE_LIMIT as i64 {
            return UNWIND;
        }
        let frame = amb_sequence_new(0, 4);
        amb_sequence_set(frame, 0, definition);
        for (index, number) in [(1, position as i64), (3, length + 1)] {
            let number = amb_int_new(number);
            amb_sequence_set(frame, index, number);
            amb_release(number);
        }
        if inner != 0 {
            amb_sequence_set(frame, 2, inner);
            amb_release(inner);
        }
        (*context).trace = frame;
        UNWIND
    }
}

/// Takes the trace of the unwinding that is ending, as the list an error
/// holds: a record for each frame, innermost first.
pub(crate) unsafe fn take(context: *mut Context) -> Word {
    unsafe {
        let chain = core::mem::take(&mut (*context).trace);
        let length = if chain == 0 { 0 } else { int(words(chain)[3]) };
        let list = amb_sequence_new(1, length as u64);
        let mut frame = chain;
        // The chain starts at the outermost frame, which goes last.
        for index in (0..length as u64).rev() {
            let [definition, position, inner, _] = *words(frame) else {
                break;
            };
            crate::object::amb_retain(position);
            put(list, index, definition, position);
            frame = inner;
        }
        amb_release(chain);
        list
    }
}

/// Puts the frame for `position`, an integer given up, of `definition`,
/// a string, in a list of frames.
unsafe fn put(list: Word, index: u64, definition: Word, position: Word) {
    unsafe {
        let record = amb_record_new(2);
        crate::object::amb_retain(definition);
        set(record, "definition", definition);
        set(record, "position", position);
        amb_sequence_set(list, index, record);
        amb_release(record);
    }
}

/// The trace gathered so far, taken out of the context: for an unwinding
/// that is set aside. Null if there is none.
pub(crate) unsafe fn detach(context: *mut Context) -> Word {
    let chain = unsafe { core::mem::take(&mut (*context).trace) };
    if chain == 0 { NULL } else { chain }
}

/// Puts back a trace that [`detach`] took, in place of any gathered
/// since.
pub(crate) unsafe fn attach(context: *mut Context, chain: Word) {
    unsafe {
        amb_release(core::mem::take(&mut (*context).trace));
        (*context).trace = if chain == NULL { 0 } else { chain };
    }
}
