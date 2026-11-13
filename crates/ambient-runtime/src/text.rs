//! Strings, and the interned strings that name record fields.

use alloc::vec::Vec;

use crate::state::Shared;

use crate::object::{KIND_STRING, KIND_TAG, boolean, bytes, object, payload, pin};
use crate::{NULL, Word};

/// A string that is never reclaimed, copied from `length` bytes at
/// `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_pinned(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        pin(word);
        core::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// A string copied from `length` bytes at `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_new(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        core::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// The one tag with this text. Two tags are the same tag exactly when
/// their words are equal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_tag(pointer: *const u8, length: u64) -> Word {
    static TAGS: Shared<Vec<usize>> = Shared::new(Vec::new());
    unsafe {
        let text = core::slice::from_raw_parts(pointer, length as usize);
        TAGS.with(|tags| {
            if let Some(known) = tags.iter().find(|known| bytes(**known as Word) == text) {
                return *known as Word;
            }
            let word = object(KIND_TAG, length as usize);
            pin(word);
            core::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
            tags.push(word as usize);
            word
        })
    }
}

/// The one string with these bytes that stands for a field name. Two field
/// names are the same name exactly when their words are equal.
///
/// TODO: an interned name is never given back, so a program that keeps
/// loading code with new field names grows by them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_intern(pointer: *const u8, length: u64) -> Word {
    static NAMES: Shared<Vec<usize>> = Shared::new(Vec::new());
    unsafe {
        let name = core::slice::from_raw_parts(pointer, length as usize);
        NAMES.with(|names| {
            if let Some(known) = names.iter().find(|known| bytes(**known as Word) == name) {
                return *known as Word;
            }
            let word = amb_string_pinned(pointer, length);
            names.push(word as usize);
            word
        })
    }
}

/// A string of `bytes`.
unsafe fn string(bytes: &[u8]) -> Word {
    unsafe { amb_string_new(bytes.as_ptr(), bytes.len() as u64) }
}

/// Whether one string comes before another, by their bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_less(left: Word, right: Word) -> Word {
    unsafe { boolean(bytes(left) < bytes(right)) }
}

/// A string's size in bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_length(value: Word) -> Word {
    unsafe { crate::amb_int_new(bytes(value).len() as i64) }
}

/// One string followed by another.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_join(left: Word, right: Word) -> Word {
    unsafe { string(&[bytes(left), bytes(right)].concat()) }
}

/// The string between two positions, or null if they are out of range,
/// out of order, or inside a character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_slice(value: Word, start: Word, end: Word) -> Word {
    unsafe {
        // Every string is UTF-8: a literal, a host's string, or what
        // these operations made of them.
        let text = core::str::from_utf8_unchecked(bytes(value));
        match crate::sequence::range(start, end, text.len()) {
            Some(range)
                if text.is_char_boundary(range.start) && text.is_char_boundary(range.end) =>
            {
                string(&text.as_bytes()[range])
            }
            _ => NULL,
        }
    }
}
