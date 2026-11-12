//! Strings, and the interned strings that name record fields.

use std::sync::{Mutex, PoisonError};

use crate::Word;
use crate::object::{KIND_STRING, bytes, object, payload, pin};

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

/// The one string with these bytes that stands for a field name. Two field
/// names are the same name exactly when their words are equal.
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
