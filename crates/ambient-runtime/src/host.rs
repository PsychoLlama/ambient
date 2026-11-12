//! For the host: making a context and reading results.

use crate::object::layout::DEFINITION_HASH;
use crate::object::{
    KIND_CELL, KIND_CLOSURE, KIND_FLOAT, KIND_LIST, KIND_RECORD, KIND_STRING, KIND_TAG, KIND_TOKEN,
    KIND_TUPLE, header, int, payload, words,
};
use crate::task::Scheduler;
use crate::{Context, FALSE, Kind, NULL, TRUE, Word};

/// A new context. Free it with [`amb_context_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_new() -> *mut Context {
    Box::into_raw(Box::new(Context {
        entries: 0,
        fault: 0,
        abort_token: 0,
        abort_args: 0,
        native: 0,
        host: 0,
        mode: crate::RUNNING,
        scheduler: Box::into_raw(Box::new(Scheduler::new())) as u64,
        trace: 0,
    }))
}

/// Frees a context.
///
/// The entry must have returned for good: with a value, or unwinding in a
/// mode other than suspending. Every task is over by then. A context
/// freed while tasks are suspended drops them where they wait, without
/// their cleanups, and leaks what they held.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_free(context: *mut Context) {
    unsafe {
        crate::abort::abandon(context);
        drop(Box::from_raw(
            (*context).scheduler as usize as *mut Scheduler,
        ));
        drop(Box::from_raw(context));
    }
}

/// Whether tasks are running, suspending, or resuming: what a host reads
/// after the entry unwinds, to see whether to call [`crate::amb_schedule`]
/// and then the entry again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_mode(context: *mut Context) -> u64 {
    unsafe { (*context).mode }
}

/// The [`Fault`] code of the last unwinding in this context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_fault(context: *mut Context) -> u64 {
    unsafe { (*context).fault }
}

/// The [`Kind`] of a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_kind(word: Word) -> u64 {
    let kind = if word & 1 == 1 {
        Kind::Int
    } else if word == NULL {
        Kind::Null
    } else if word == TRUE || word == FALSE {
        Kind::Bool
    } else {
        match unsafe { (*header(word)).kind } {
            KIND_FLOAT => Kind::Float,
            KIND_STRING => Kind::String,
            KIND_TUPLE => Kind::Tuple,
            KIND_LIST => Kind::List,
            KIND_RECORD => Kind::Record,
            KIND_CLOSURE => Kind::Function,
            KIND_CELL => Kind::Cell,
            KIND_TAG => Kind::Tag,
            KIND_TOKEN => Kind::Token,
            _ => Kind::Int,
        }
    };
    kind as u64
}

/// Room for `count` words that is never reclaimed, zeroed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_words(count: u64) -> *mut u64 {
    Box::leak(vec![0u64; count as usize].into_boxed_slice()).as_mut_ptr()
}

/// The integer a value holds. The value must be an integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_value(word: Word) -> i64 {
    unsafe { int(word) }
}

/// The bits of the float a value holds. The value must be a float.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_bits(word: Word) -> u64 {
    unsafe { payload(word).read() }
}

/// How many members a tuple or list has, fields a record has, or bytes a
/// string has.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_length(word: Word) -> u64 {
    unsafe {
        let size = (*header(word)).size;
        match (*header(word)).kind {
            KIND_STRING | KIND_TAG => size,
            KIND_RECORD => size / 16,
            _ => size / 8,
        }
    }
}

/// The address of a string's bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_bytes(word: Word) -> *const u8 {
    unsafe { payload(word).cast() }
}

/// The word at `index` of an object's payload, borrowed: a member of a
/// tuple or list, or, for a record, a key at an even index and its value
/// at the next.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_word_at(word: Word, index: u64) -> Word {
    unsafe { words(word)[index as usize] }
}

/// The hash of the definition a function value is over, as a string,
/// borrowed. The values it captured are the words of its payload after
/// the first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_hash(word: Word) -> Word {
    unsafe {
        let definition = words(word)[0] as usize as *const u64;
        definition.add(DEFINITION_HASH as usize / 8).read()
    }
}

/// How many objects the calling thread has made and not yet reclaimed. A
/// host that has released everything it was given reads the same number
/// before and after a call, or the program leaked.
#[unsafe(no_mangle)]
pub extern "C" fn amb_live() -> u64 {
    crate::object::live()
}
