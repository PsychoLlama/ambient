//! Tuples and lists.

use core::ops::Range;

use crate::object::{KIND_LIST, KIND_TUPLE, amb_retain, boolean, int, kind, object, words};
use crate::{NULL, Word};

/// A tuple, or a list if `list` is not zero, of `length` nulls. Fill it
/// with [`amb_sequence_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_new(list: u64, length: u64) -> Word {
    unsafe {
        let kind = if list == 0 { KIND_TUPLE } else { KIND_LIST };
        let word = object(kind, length as usize * 8);
        words(word).fill(NULL);
        word
    }
}

/// Puts `value` at `index` of a sequence under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_set(sequence: Word, index: u64, value: Word) {
    unsafe {
        amb_retain(value);
        words(sequence)[index as usize] = value;
    }
}

/// The member at `index`, which must exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_get(sequence: Word, index: u64) -> Word {
    unsafe {
        let value = words(sequence)[index as usize];
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

/// A list of `members`, each of which it takes a reference to.
unsafe fn list(members: &[Word]) -> Word {
    unsafe {
        let word = object(KIND_LIST, members.len() * 8);
        for (slot, member) in words(word).iter_mut().zip(members) {
            amb_retain(*member);
            *slot = *member;
        }
        word
    }
}

/// How many members a list, or a tuple used as one, has.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_length(value: Word) -> Word {
    unsafe { crate::amb_int_new(words(value).len() as i64) }
}

/// The member at a position, or null if there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_get(value: Word, position: Word) -> Word {
    unsafe {
        let position = usize::try_from(int(position)).ok();
        match position.and_then(|position| words(value).get(position)) {
            Some(member) => {
                amb_retain(*member);
                *member
            }
            None => NULL,
        }
    }
}

/// A list of the members of one, then `member`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_push(value: Word, member: Word) -> Word {
    unsafe { list(&[&*words(value), &[member]].concat()) }
}

/// A list of the members of one, then those of another.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_join(left: Word, right: Word) -> Word {
    unsafe { list(&[&*words(left), &*words(right)].concat()) }
}

/// The members between two positions, or null if the positions are out
/// of range or out of order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_slice(value: Word, start: Word, end: Word) -> Word {
    unsafe {
        let members = words(value);
        match range(start, end, members.len()) {
            Some(range) => list(&members[range]),
            None => NULL,
        }
    }
}
