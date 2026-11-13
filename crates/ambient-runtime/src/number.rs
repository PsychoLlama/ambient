//! Integers, floats, and structural equality.

use alloc::vec;

use crate::list::members;
use crate::object::{
    KIND_CLOSURE, KIND_FLOAT, KIND_INT, KIND_ITERATOR, KIND_LIST, KIND_METHOD, KIND_RECORD,
    KIND_SCOPE, KIND_STRING, KIND_TUPLE, boolean, bytes, fault, float, header, int, is_object,
    object, payload, small, words,
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
                (KIND_STRING, KIND_STRING) => bytes(left) == bytes(right),
                // A record's keys are interned and kept in order, so equal
                // records have equal keys at equal positions.
                (KIND_LIST, KIND_LIST) => all(members(left), members(right)),
                (KIND_TUPLE, KIND_TUPLE)
                | (KIND_RECORD, KIND_RECORD)
                | (KIND_ITERATOR, KIND_ITERATOR) => all(words(left), words(right)),
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
