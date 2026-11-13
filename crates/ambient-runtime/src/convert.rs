//! Conversions between integers, floats, and text. See "Conversions" in
//! `spec/operations.md`.

use alloc::format;
use alloc::string::ToString;

use crate::number::{amb_float_new, amb_int_new};
use crate::object::{bytes, float, int};
use crate::text::amb_string_new;
use crate::{NULL, Word};

/// The float nearest an integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_float(value: Word) -> Word {
    #[allow(clippy::cast_precision_loss)]
    unsafe {
        amb_float_new((int(value) as f64).to_bits())
    }
}

/// A float without its fraction, or null if that is no integer: the
/// float is not a number, or is out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_int(value: Word) -> Word {
    /// Two to the sixty-third: the first float past every integer.
    const PAST: f64 = 9_223_372_036_854_775_808.0;
    unsafe {
        // A float this large has no fraction, so one in range before
        // its fraction is dropped is in range after; the cast drops it.
        let float = float(value);
        if (-PAST..PAST).contains(&float) {
            #[allow(clippy::cast_possible_truncation)]
            amb_int_new(float as i64)
        } else {
            NULL
        }
    }
}

unsafe fn text(text: &str) -> Word {
    unsafe { amb_string_new(text.as_ptr(), text.len() as u64) }
}

/// An integer in decimal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_text(value: Word) -> Word {
    unsafe { text(&int(value).to_string()) }
}

/// A float as the shortest decimal that reads back as the same float.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_text(value: Word) -> Word {
    unsafe { text(&format!("{:?}", float(value))) }
}

/// What a string says, every byte of it, or nothing.
unsafe fn read<T: core::str::FromStr>(value: Word) -> Option<T> {
    unsafe { core::str::from_utf8(bytes(value)).ok()?.parse().ok() }
}

/// The integer a string is, in decimal, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_int(value: Word) -> Word {
    unsafe { read(value).map_or(NULL, |value| amb_int_new(value)) }
}

/// The float a string is, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_float(value: Word) -> Word {
    unsafe { read::<f64>(value).map_or(NULL, |value| amb_float_new(value.to_bits())) }
}
