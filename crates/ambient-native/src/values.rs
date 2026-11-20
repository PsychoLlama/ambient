//! The few things nearly every function of a bundle does, over Rust's
//! own slices in place of a pointer and a length.

use crate::{
    Call, Word, amb_bundle_fail, amb_bytes_new, amb_length, amb_string_bytes, amb_string_new,
};

/// The bytes of a string, a tag, or a bytes value, where they lie.
///
/// # Safety
///
/// `value` is one of those, held for as long as the bytes are read.
#[must_use]
pub unsafe fn text<'a>(value: Word) -> &'a [u8] {
    // SAFETY: the value's bytes are as many as its length, and stay
    // while it is held.
    unsafe {
        let length = usize::try_from(amb_length(value)).unwrap_or(0);
        core::slice::from_raw_parts(amb_string_bytes(value), length)
    }
}

/// A string value of `text`, which the caller owns.
#[must_use]
pub fn string(text: &str) -> Word {
    // SAFETY: the pointer is to as many bytes as the length says.
    unsafe { amb_string_new(text.as_ptr(), text.len() as u64) }
}

/// A bytes value of `bytes`, which the caller owns.
#[must_use]
pub fn bytes(bytes: &[u8]) -> Word {
    // SAFETY: the pointer is to as many bytes as the length says.
    unsafe { amb_bytes_new(bytes.as_ptr(), bytes.len() as u64) }
}

/// Says the call failed, and gives what a function that failed gives.
///
/// # Safety
///
/// `call` is what the function running was handed.
pub unsafe fn fail(call: *mut Call, message: &str) -> Word {
    // SAFETY: the caller's to uphold; the message is as long as said.
    unsafe { amb_bundle_fail(call, message.as_ptr(), message.len() as u64) };
    crate::NULL
}
