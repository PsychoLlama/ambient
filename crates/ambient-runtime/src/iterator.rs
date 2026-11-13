//! Iterators: a function that steps, or something the runtime steps
//! itself. See `spec/iteration.md`.
//!
//! An iterator is an object of three words: which kind it is, and two
//! values that kind keeps. Every word is a value, so an iterator is
//! held, compared, and reclaimed like a tuple.

use crate::list::members;
use crate::object::{KIND_ITERATOR, KIND_TUPLE, amb_retain, bytes, int, object, small, words};
use crate::{NULL, Word, amb_int_new};

/// Over a function: the function, which is called for each step.
const FUNCTION: i64 = 0;
/// Over a list: the list, and the position of the next member.
const LIST: i64 = 1;
/// Over integers: the next one, and the one to stop before.
const RANGE: i64 = 2;
/// Over a string: the string, and the position of the next character.
const STRING: i64 = 3;

/// An iterator of `kind` that owns `first` and `second`.
unsafe fn iterator(kind: i64, first: Word, second: Word) -> Word {
    unsafe {
        let word = object(KIND_ITERATOR, 24);
        // A kind is small, so it is always immediate.
        words(word).copy_from_slice(&[small(kind).unwrap_or(NULL), first, second]);
        word
    }
}

unsafe fn kind(iterator: Word) -> i64 {
    unsafe { int(words(iterator)[0]) }
}

/// What a step gives when something is left: `item`, which it owns, and
/// the iterator of the rest.
unsafe fn step(item: Word, rest: Word) -> Word {
    unsafe {
        let word = object(KIND_TUPLE, 16);
        words(word).copy_from_slice(&[item, rest]);
        word
    }
}

/// An iterator that `function` steps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_iterator_new(function: Word) -> Word {
    unsafe {
        amb_retain(function);
        iterator(FUNCTION, function, NULL)
    }
}

/// An iterator over the members of a list, from the first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_iterate(list: Word) -> Word {
    unsafe {
        amb_retain(list);
        iterator(LIST, list, amb_int_new(0))
    }
}

/// An iterator over the characters of a string, from the first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_iterate(string: Word) -> Word {
    unsafe {
        amb_retain(string);
        iterator(STRING, string, amb_int_new(0))
    }
}

/// An iterator over the integers from `start` up to `end`, which it
/// leaves out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_range(start: Word, end: Word) -> Word {
    unsafe {
        amb_retain(start);
        amb_retain(end);
        iterator(RANGE, start, end)
    }
}

/// The function that steps an iterator, or null if the runtime steps
/// it: see [`amb_iterator_advance`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_iterator_stepper(iterator: Word) -> Word {
    unsafe {
        if kind(iterator) != FUNCTION {
            return NULL;
        }
        let function = words(iterator)[1];
        amb_retain(function);
        function
    }
}

/// A step of an iterator the runtime steps: null if nothing is left,
/// or else a tuple of an item and the iterator of the rest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_iterator_advance(iterator: Word) -> Word {
    unsafe {
        let held = words(iterator);
        match kind(iterator) {
            LIST => {
                let position = int(held[2]);
                let Some(item) = members(held[1]).get(position as usize) else {
                    return NULL;
                };
                amb_retain(*item);
                amb_retain(held[1]);
                step(
                    *item,
                    self::iterator(LIST, held[1], amb_int_new(position + 1)),
                )
            }
            RANGE => {
                let start = int(held[1]);
                if start >= int(held[2]) {
                    return NULL;
                }
                amb_retain(held[2]);
                let rest = self::iterator(RANGE, amb_int_new(start + 1), held[2]);
                step(amb_int_new(start), rest)
            }
            STRING => {
                let position = int(held[2]) as usize;
                // Every string is UTF-8, and the position is between
                // two of its characters.
                let text = core::str::from_utf8_unchecked(bytes(held[1]));
                let Some(character) = text[position..].chars().next() else {
                    return NULL;
                };
                let end = position + character.len_utf8();
                amb_retain(held[1]);
                step(
                    crate::text::string(&text.as_bytes()[position..end]),
                    self::iterator(STRING, held[1], amb_int_new(end as i64)),
                )
            }
            _ => NULL,
        }
    }
}

/// What a test of an iterator against a type looks at.
pub(crate) enum Tested {
    /// The function that steps it: a function says what type it is of.
    Function(Word),
    /// Items it is known to give, each of which is tested.
    Items(&'static [u64]),
}

/// What an iterator gives, as far as that is known without stepping it.
pub(crate) unsafe fn tested(iterator: Word) -> Tested {
    unsafe {
        let held = words(iterator);
        match kind(iterator) {
            FUNCTION => Tested::Function(held[1]),
            LIST => {
                let members = members(held[1]);
                let from = (int(held[2]) as usize).min(members.len());
                Tested::Items(&members[from..])
            }
            // Every item is a string, so the string stands for all.
            STRING if (int(held[2]) as usize) < bytes(held[1]).len() => Tested::Items(&held[1..2]),
            // Every item is an integer, so the first stands for all.
            RANGE if int(held[1]) < int(held[2]) => Tested::Items(&held[1..2]),
            _ => Tested::Items(&[]),
        }
    }
}
