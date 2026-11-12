//! Strings, and the interned strings that name record fields.

use std::sync::{Mutex, PoisonError};

use crate::Word;
use crate::object::{KIND_STRING, KIND_TAG, bytes, object, payload, pin};

/// A string that is never reclaimed, copied from `length` bytes at
/// `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_pinned(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        pin(word);
        std::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// A string copied from `length` bytes at `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_new(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        std::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// The one tag with this text. Two tags are the same tag exactly when
/// their words are equal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_tag(pointer: *const u8, length: u64) -> Word {
    static TAGS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    unsafe {
        let text = std::slice::from_raw_parts(pointer, length as usize);
        let mut tags = TAGS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = tags.iter().find(|known| bytes(**known as Word) == text) {
            return *known as Word;
        }
        let word = object(KIND_TAG, length as usize);
        pin(word);
        std::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        tags.push(word as usize);
        word
    }
}

/// The one string with these bytes that stands for a field name. Two field
/// names are the same name exactly when their words are equal.
///
/// TODO: an interned name is never given back, so a program that keeps
/// loading code with new field names grows by them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_intern(pointer: *const u8, length: u64) -> Word {
    static NAMES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    unsafe {
        let name = std::slice::from_raw_parts(pointer, length as usize);
        let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = names.iter().find(|known| bytes(**known as Word) == name) {
            return *known as Word;
        }
        let word = amb_string_pinned(pointer, length);
        names.push(word as usize);
        word
    }
}
