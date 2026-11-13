//! Tuples, and what tuples and lists share. Lists are in `list.rs`.

use core::ops::Range;

use crate::list;
use crate::object::{KIND_LIST, KIND_TUPLE, amb_retain, boolean, int, kind, object, words};
use crate::{NULL, Word};

/// A tuple, or a list if `list` is not zero, of `length` nulls. Fill it
/// with [`amb_sequence_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_new(list: u64, length: u64) -> Word {
    unsafe {
        if list != 0 {
            return list::filled(length as usize);
        }
        let word = object(KIND_TUPLE, length as usize * 8);
        words(word).fill(NULL);
        word
    }
}

/// Puts `value` at `index` of a sequence under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_set(sequence: Word, index: u64, value: Word) {
    unsafe {
        amb_retain(value);
        list::members_mut(sequence)[index as usize] = value;
    }
}

/// The member at `index`, which must exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_get(sequence: Word, index: u64) -> Word {
    unsafe {
        let value = list::members(sequence)[index as usize];
        amb_retain(value);
        value
    }
}

/// Whether a value is a tuple of exactly `length` members, or, if `list`
/// is not zero, a list of any length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_is(value: Word, list: u64, length: u64) -> Word {
    unsafe {
        boolean(match kind(value) {
            KIND_TUPLE => list == 0 && words(value).len() as u64 == length,
            KIND_LIST => list != 0,
            _ => false,
        })
    }
}

/// The positions from `start` to `end`, two integers, if they are in
/// order and within something `length` long.
pub(crate) unsafe fn range(start: Word, end: Word, length: usize) -> Option<Range<usize>> {
    let (start, end) = unsafe { (int(start), int(end)) };
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok().filter(|end| *end <= length)?;
    (start <= end).then_some(start..end)
}
