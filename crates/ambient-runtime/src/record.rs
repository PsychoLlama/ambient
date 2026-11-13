//! Records.
//!
//! A record is its fields as (key, value) pairs, ordered by key. A key is
//! an interned string, so the order is by address: arbitrary, but the same
//! for every record in one execution.
//!
//! A method (`spec/methods.md`) is held as a closure of its own kind,
//! with room for the record it will receive. Reading the field gives a
//! copy that holds the record, so no record holds itself.

use crate::object::{
    KIND_CLOSURE, KIND_METHOD, KIND_RECORD, amb_retain, boolean, header, kind, object, words,
};
use crate::unit::{self, amb_unit_retain};
use crate::{NULL, Word};

/// A record with room for `length` fields. Fill it with
/// [`amb_record_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_new(length: u64) -> Word {
    unsafe {
        let word = object(KIND_RECORD, length as usize * 16);
        words(word).fill(0);
        word
    }
}

/// Adds a field to a record under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_set(record: Word, key: Word, value: Word) {
    unsafe {
        amb_retain(value);
        let fields = words(record);
        // Unfilled pairs have a zero key and sit at the end.
        let filled = fields.chunks(2).take_while(|pair| pair[0] != 0).count();
        let position = fields[..filled * 2]
            .chunks(2)
            .position(|pair| pair[0] > key)
            .unwrap_or(filled);
        fields.copy_within(position * 2..filled * 2, position * 2 + 2);
        fields[position * 2] = key;
        fields[position * 2 + 1] = value;
    }
}

pub(crate) unsafe fn find(record: Word, key: Word) -> Option<Word> {
    unsafe {
        words(record)
            .chunks(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    }
}

/// Where among a method's words the record it receives goes: after its
/// definition and the function it calls.
const RECEIVER: usize = 2;

/// Makes a closure that nothing else holds yet a method, for a record
/// to hold.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_method(closure: Word) -> Word {
    unsafe {
        amb_method_mark(closure);
        amb_retain(closure);
        closure
    }
}

/// [`amb_method`], for a host that holds the closure already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_method_mark(closure: Word) {
    unsafe { (*header(closure)).kind = KIND_METHOD }
}

/// What reading `held` out of `record` gives, which the caller owns:
/// the value, or for a method the function bound to the record.
pub(crate) unsafe fn read(record: Word, held: Word) -> Word {
    unsafe {
        if kind(held) != KIND_METHOD {
            amb_retain(held);
            return held;
        }
        let method = words(held);
        let word = object(KIND_CLOSURE, method.len() * 8);
        let bound = words(word);
        bound.copy_from_slice(method);
        bound[RECEIVER] = record;
        amb_unit_retain(unit::of(bound[0]));
        bound[1..].iter().for_each(|value| amb_retain(*value));
        word
    }
}

/// The field `key` of a record, which must have it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_get(record: Word, key: Word) -> Word {
    unsafe { read(record, find(record, key).unwrap_or(NULL)) }
}

/// Whether a value is a record with the field `key`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_has(value: Word, key: Word) -> Word {
    unsafe { boolean(kind(value) == KIND_RECORD && find(value, key).is_some()) }
}

/// Whether a value is a record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_is(value: Word) -> Word {
    unsafe { boolean(kind(value) == KIND_RECORD) }
}

/// A record with every field of `base` and of `overlay`, taking the
/// overlay's value where both have a field. `base` may be null, for no
/// fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_merge(base: Word, overlay: Word) -> Word {
    unsafe {
        if base == NULL {
            amb_retain(overlay);
            return overlay;
        }
        // Both are ordered by key, so this is a merge of two sorted runs.
        let (below, above) = (words(base), words(overlay));
        let shared = below
            .chunks(2)
            .filter(|pair| find(overlay, pair[0]).is_some())
            .count();
        let length = below.len() / 2 + above.len() / 2 - shared;
        let word = object(KIND_RECORD, length * 16);
        let fields = words(word);
        let (mut low, mut high) = (0, 0);
        for pair in fields.chunks_mut(2) {
            let from_base = high == above.len() || (low < below.len() && below[low] < above[high]);
            let (from, source) = if from_base {
                low += 2;
                (base, &below[low - 2..low])
            } else {
                if low < below.len() && below[low] == above[high] {
                    low += 2;
                }
                high += 2;
                (overlay, &above[high - 2..high])
            };
            // A method stays bound to the record it had.
            pair[0] = source[0];
            pair[1] = read(from, source[1]);
        }
        word
    }
}
