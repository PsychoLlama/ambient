//! Integers, floats, and structural equality.

use alloc::vec;

use crate::list::members;
use crate::object::{
    KIND_BYTES, KIND_CLOSURE, KIND_FLOAT, KIND_HANDLE, KIND_INT, KIND_ITERATOR, KIND_LIST,
    KIND_METHOD, KIND_RECORD, KIND_SCOPE, KIND_STRING, KIND_TUPLE, boolean, bytes, fault, float,
    header, int, is_object, object, payload, small, words,
};
use crate::{Context, Fault, Word};

/// An integer of any size as a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_new(value: i64) -> Word {
    small(value).unwrap_or_else(|| unsafe {
        let word = object(KIND_INT, 8);
        payload(word).cast::<i64>().write(value);
        word
    })
}

/// A float, given as its bits, as a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_new(bits: u64) -> Word {
    unsafe {
        let word = object(KIND_FLOAT, 8);
        payload(word).write(bits);
        word
    }
}

// Generic over the operation, so each gets a copy of its own: it is a
// few instructions, and a call through a pointer would be most of them.
pub(crate) unsafe fn arithmetic(
    context: *mut Context,
    left: Word,
    right: Word,
    operation: impl Fn(i64, i64) -> Option<i64>,
) -> Word {
    unsafe {
        match operation(int(left), int(right)) {
            Some(result) => amb_int_new(result),
            None => fault(context, Fault::Overflow),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_add(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_add) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_subtract(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_sub) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_multiply(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_mul) }
}

/// Division that truncates toward zero. Dividing by zero is its own
/// fault; the one quotient that leaves the range is an overflow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_divide(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { divided(context, left, right, i64::checked_div) }
}

/// The remainder of that division, which has the sign of `left`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_remainder(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { divided(context, left, right, i64::checked_rem) }
}

/// `base` to the power `exponent`. A negative power is one divided by
/// the positive power, truncated as division is, so zero has none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_power(context: *mut Context, base: Word, exponent: Word) -> Word {
    unsafe {
        if int(base) == 0 && int(exponent) < 0 {
            return fault(context, Fault::DivideByZero);
        }
        arithmetic(context, base, exponent, power)
    }
}

/// `base` to the power `exponent`, or nothing if it leaves the range,
/// for any base but zero to a negative power.
fn power(base: i64, exponent: i64) -> Option<i64> {
    let odd = exponent % 2 != 0;
    match (base, exponent) {
        (_, 0) | (1, _) => Some(1),
        (-1, _) => Some(if odd { -1 } else { 1 }),
        // What is between zero and one truncates to zero.
        (_, ..0) | (0, _) => Some(0),
        // A power past `u32` of any other base is out of range.
        _ => base.checked_pow(u32::try_from(exponent).ok()?),
    }
}

unsafe fn divided(
    context: *mut Context,
    left: Word,
    right: Word,
    operation: fn(i64, i64) -> Option<i64>,
) -> Word {
    unsafe {
        if int(right) == 0 {
            return fault(context, Fault::DivideByZero);
        }
        arithmetic(context, left, right, operation)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_less(left: Word, right: Word) -> Word {
    unsafe { boolean(int(left) < int(right)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_add(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) + float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_subtract(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) - float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_multiply(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) * float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_divide(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) / float(right)).to_bits()) }
}

/// `base` to the power `exponent`, as C's `pow` says.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_power(base: Word, exponent: Word) -> Word {
    unsafe { amb_float_new(libm::pow(float(base), float(exponent)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_less(left: Word, right: Word) -> Word {
    unsafe { boolean(float(left) < float(right)) }
}

/// Structural equality.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_equal(left: Word, right: Word) -> Word {
    unsafe { boolean(equal(left, right)) }
}

pub(crate) unsafe fn equal(left: Word, right: Word) -> bool {
    // Most comparisons are of words that are the same or of ones that
    // are no object, and are over before anything is queued.
    if left == right {
        return true;
    }
    if !is_object(left) || !is_object(right) {
        return false;
    }
    // A value can be nested arbitrarily deep, so the pairs still to be
    // compared are queued here, never compared by recursion.
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        if left == right {
            continue;
        }
        if !is_object(left) || !is_object(right) {
            return false;
        }
        let mut all = |left: &[u64], right: &[u64]| {
            pending.extend(left.iter().copied().zip(right.iter().copied()));
            left.len() == right.len()
        };
        let equal = unsafe {
            match ((*header(left)).kind, (*header(right)).kind) {
                (KIND_INT, KIND_INT) => int(left) == int(right),
                // Equality is structural: a float equals itself, bit for bit.
                (KIND_FLOAT, KIND_FLOAT) => payload(left).read() == payload(right).read(),
                (KIND_STRING, KIND_STRING) | (KIND_BYTES, KIND_BYTES) => {
                    bytes(left) == bytes(right)
                }
                // A record's keys are interned and kept in order, so equal
                // records have equal keys at equal positions.
                (KIND_LIST, KIND_LIST) => all(members(left), members(right)),
                (KIND_TUPLE, KIND_TUPLE)
                | (KIND_RECORD, KIND_RECORD)
                | (KIND_ITERATOR, KIND_ITERATOR) => all(words(left), words(right)),
                // The same handle is the same brand and number, which
                // are not values.
                (KIND_HANDLE, KIND_HANDLE) => words(left)[..2] == words(right)[..2],
                (KIND_CLOSURE, KIND_CLOSURE) | (KIND_METHOD, KIND_METHOD) => {
                    let (left, right) = (words(left), words(right));
                    crate::closure::same_definition(left[0], right[0])
                        && all(&left[1..], &right[1..])
                }
                // Functions made at the same types are made at equal ones.
                (KIND_SCOPE, KIND_SCOPE) => crate::scope::same(left, right),
                _ => false,
            }
        };
        if !equal {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::power;

    #[test]
    fn a_negative_power_truncates_as_division_does() {
        assert_eq!(power(2, -1), Some(0));
        assert_eq!(power(-1, -3), Some(-1));
        assert_eq!(power(1, i64::MIN), Some(1));
    }

    #[test]
    fn a_power_out_of_range_is_nothing() {
        assert_eq!(power(2, 63), None);
        assert_eq!(power(-2, 63), Some(i64::MIN));
        assert_eq!(power(2, i64::MAX), None);
        assert_eq!(power(0, i64::MAX), Some(0));
    }

    proptest! {
        #[test]
        fn a_power_is_repeated_multiplication(base in -50i64..50, exponent in 0i64..12) {
            let multiplied = (0..exponent).try_fold(1i64, |product, _| product.checked_mul(base));
            prop_assert_eq!(power(base, exponent), multiplied);
        }
    }
}
