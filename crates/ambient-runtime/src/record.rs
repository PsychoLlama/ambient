//! Records.
//!
//! A record is its fields as (key, value) pairs, ordered by key. A key is
//! an interned string, so the order is by address: arbitrary, but the same
//! for every record in one execution.

use crate::object::{KIND_RECORD, amb_retain, boolean, kind, object, words};
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

/// The field `key` of a record, which must have it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_get(record: Word, key: Word) -> Word {
    unsafe {
        let value = find(record, key).unwrap_or(NULL);
        amb_retain(value);
        value
    }
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
            let source = if from_base {
                low += 2;
                &below[low - 2..low]
            } else {
                if low < below.len() && below[low] == above[high] {
                    low += 2;
                }
                high += 2;
                &above[high - 2..high]
            };
            amb_retain(source[1]);
            pair.copy_from_slice(source);
        }
        word
    }
}
