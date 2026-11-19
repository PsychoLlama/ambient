//! Conversions between integers, floats, and text. See "Conversions" in
//! `docs/spec/operations.md`.

use crate::number::{amb_float_new, amb_int_new};
use crate::object::{bytes, float, int};
use crate::shortest;
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
    let mut digits = [0; DIGITS];
    let digits = decimal(unsafe { int(value) }, &mut digits);
    unsafe { amb_string_new(digits.as_ptr(), digits.len() as u64) }
}

/// The most bytes an integer takes in decimal: its sign and its digits.
const DIGITS: usize = 20;

/// `value` in decimal, written at the end of `room`.
///
/// Written out here because the standard library's formatting, which
/// would do it, is several thousand bytes of a WebAssembly module that
/// only turns a number into text.
fn decimal(value: i64, room: &mut [u8; DIGITS]) -> &[u8] {
    let mut left = value.unsigned_abs();
    let mut at = DIGITS;
    loop {
        at -= 1;
        #[allow(clippy::cast_possible_truncation)]
        let digit = (left % 10) as u8;
        room[at] = b'0' + digit;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        room[at] = b'-';
    }
    &room[at..]
}

/// A float as the shortest decimal that reads back as the same float.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_text(value: Word) -> Word {
    let mut room = [0; shortest::ROOM];
    let written = shortest::shortest(unsafe { float(value) }, &mut room);
    unsafe { amb_string_new(written.as_ptr(), written.len() as u64) }
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

/// The scalar value of the one character a string holds, or null if it
/// holds none or more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_code(value: Word) -> Word {
    unsafe {
        let mut characters = core::str::from_utf8(bytes(value)).unwrap_or("").chars();
        match (characters.next(), characters.next()) {
            (Some(only), None) => amb_int_new(i64::from(u32::from(only))),
            _ => NULL,
        }
    }
}

/// The character of a scalar value, or null if the integer is not one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_code_string(value: Word) -> Word {
    unsafe {
        let scalar = u32::try_from(int(value)).ok().and_then(char::from_u32);
        scalar.map_or(NULL, |scalar| text(scalar.encode_utf8(&mut [0; 4])))
    }
}

/// The float a string is, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_float(value: Word) -> Word {
    unsafe { read::<f64>(value).map_or(NULL, |value| amb_float_new(value.to_bits())) }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use proptest::prelude::*;

    use super::{DIGITS, decimal};

    fn written(value: i64) -> alloc::string::String {
        alloc::string::String::from_utf8_lossy(decimal(value, &mut [0; DIGITS])).into_owned()
    }

    #[test]
    fn the_longest_integers_fit() {
        for value in [i64::MIN, i64::MAX, 0] {
            assert_eq!(written(value), value.to_string());
        }
    }

    proptest! {
        #[test]
        fn an_integer_in_decimal_is_what_the_standard_library_writes(value: i64) {
            prop_assert_eq!(written(value), value.to_string());
        }
    }
}
