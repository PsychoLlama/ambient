//! Bytes: a run of bytes that need not be text. See "Bytes" in
//! `docs/spec/operations.md`.

use alloc::vec::Vec;

use crate::list::members;
use crate::number::amb_int_new;
use crate::object::{KIND_BYTES, bytes, int, object, payload, pin};
use crate::{NULL, Word};

/// A bytes value copied from `length` bytes at `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_new(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_BYTES, length as usize);
        core::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// A bytes value that is never reclaimed, copied from `length` bytes
/// at `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_pinned(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = amb_bytes_new(pointer, length);
        pin(word);
        word
    }
}

unsafe fn new(bytes: &[u8]) -> Word {
    unsafe { amb_bytes_new(bytes.as_ptr(), bytes.len() as u64) }
}

/// How many bytes a bytes value holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_length(value: Word) -> Word {
    unsafe { amb_int_new(bytes(value).len() as i64) }
}

/// The byte at a position, as an integer, or null if there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_get(value: Word, index: Word) -> Word {
    unsafe {
        let byte = usize::try_from(int(index))
            .ok()
            .and_then(|index| bytes(value).get(index));
        byte.map_or(NULL, |byte| amb_int_new(i64::from(*byte)))
    }
}

/// One bytes value followed by another.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_join(left: Word, right: Word) -> Word {
    unsafe { new(&[bytes(left), bytes(right)].concat()) }
}

/// The bytes between two positions, or null if they are out of range or
/// out of order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_slice(value: Word, start: Word, end: Word) -> Word {
    unsafe {
        let all = bytes(value);
        crate::sequence::range(start, end, all.len()).map_or(NULL, |range| new(&all[range]))
    }
}

/// The bytes of a string's UTF-8 encoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_of_string(value: Word) -> Word {
    unsafe { new(bytes(value)) }
}

/// The string some bytes encode, or null if they are not UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_text(value: Word) -> Word {
    unsafe {
        match core::str::from_utf8(bytes(value)) {
            Ok(text) => crate::text::string(text.as_bytes()),
            Err(_) => NULL,
        }
    }
}

/// The bytes a list of integers holds, or null if a member is not from
/// 0 to 255.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bytes_of_list(list: Word) -> Word {
    unsafe {
        let held: Option<Vec<u8>> = members(list)
            .iter()
            .map(|member| u8::try_from(int(*member)).ok())
            .collect();
        held.map_or(NULL, |held| new(&held))
    }
}
