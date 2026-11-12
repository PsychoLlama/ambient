//! Objects: what a word that is an address points at, and who owns it.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;

use crate::{Context, FALSE, Fault, SMALL_MAX, SMALL_MIN, TRUE, UNWIND, Word};

pub(crate) const KIND_INT: u32 = 1;
pub(crate) const KIND_FLOAT: u32 = 2;
pub(crate) const KIND_CLOSURE: u32 = 3;
pub(crate) const KIND_STRING: u32 = 4;
pub(crate) const KIND_TUPLE: u32 = 5;
pub(crate) const KIND_LIST: u32 = 6;
pub(crate) const KIND_RECORD: u32 = 7;
/// Not a value: what an abort handler is bound to.
pub(crate) const KIND_TOKEN: u32 = 8;
pub(crate) const KIND_CELL: u32 = 9;
/// A tag value: its text, interned, so that equal tags are one object.
pub(crate) const KIND_TAG: u32 = 10;
/// Not a value: the type arguments a generic function was made at.
pub(crate) const KIND_SCOPE: u32 = 12;

/// The count of an object that is never reclaimed.
pub(crate) const PINNED: u32 = u32::MAX;

/// The layout of a closure and of the definition it refers to, for
/// compiled code. Every field is one word; offsets are in bytes.
///
/// A closure is an object whose payload is its definition and then its
/// captures. A definition is not an object: it is the code, the code
/// compiled to run inside a budget, the arity, its hash, its signature,
/// and then one
/// closure per slot.
pub mod layout {
    /// From a closure to its definition.
    pub const CLOSURE_DEFINITION: i32 = 16;
    /// From a closure to its first capture.
    pub const CLOSURE_CAPTURES: i32 = 24;
    /// The words in a closure that captures nothing.
    pub const CLOSURE_WORDS: usize = 3;
    /// From a definition to its code.
    pub const DEFINITION_CODE: i32 = 0;
    /// From a definition to its code as what runs inside a budget calls
    /// it: code that charges for what it does. In a program that enters
    /// no budget this is the same code, and nothing calls it.
    pub const DEFINITION_METERED: i32 = 8;
    /// From a definition to its hash, a string: what a host names the
    /// definition of a function value by.
    pub const DEFINITION_HASH: i32 = 24;
    /// From a definition to its signature: the descriptor of its type,
    /// or zero.
    pub const DEFINITION_SIGNATURE: i32 = 32;
    /// From a definition to its first slot.
    pub const DEFINITION_SLOTS: i32 = 40;
    /// The words in a definition with no slots.
    pub const DEFINITION_WORDS: usize = 5;
}

/// Writes a closure that is never reclaimed into `words`: its header,
/// its definition, and room for `captures` values after. `words` must
/// hold [`layout::CLOSURE_WORDS`] and the captures.
pub fn pinned_closure(words: &mut [u64], definition: u64, captures: usize) {
    words[0] = u64::from(PINNED) | (u64::from(KIND_CLOSURE) << 32);
    words[1] = (captures as u64 + 1) * 8;
    words[2] = definition;
}

/// What every object starts with. The payload follows, in words.
#[repr(C)]
pub(crate) struct Header {
    pub(crate) count: u32,
    pub(crate) kind: u32,
    /// The payload's size in bytes.
    pub(crate) size: u64,
}

pub(crate) const HEADER: usize = size_of::<Header>();

pub(crate) fn layout(size: usize) -> Layout {
    // The size is one this module chose and the alignment is a constant, so
    // this cannot fail; an empty layout would only make allocation fail.
    Layout::from_size_align(HEADER + size, 8).unwrap_or(Layout::new::<Header>())
}

pub(crate) fn is_object(word: Word) -> bool {
    word != UNWIND && word.trailing_zeros() >= 3
}

pub(crate) fn header(word: Word) -> *mut Header {
    word as usize as *mut Header
}

pub(crate) unsafe fn payload(word: Word) -> *mut u64 {
    unsafe { (word as usize as *mut u8).add(HEADER).cast() }
}

thread_local! {
    /// How many objects this thread has made and not yet reclaimed or
    /// pinned. A program that returns to its host with more than it was
    /// given, besides its result, has leaked.
    static LIVE: Cell<u64> = const { Cell::new(0) };
}

/// How many objects are live. See [`crate::amb_live`].
pub(crate) fn live() -> u64 {
    LIVE.get()
}

/// Makes an object one that is never reclaimed.
pub(crate) unsafe fn pin(word: Word) {
    unsafe {
        (*header(word)).count = PINNED;
    }
    LIVE.set(LIVE.get() - 1);
}

/// A new object of `kind` with one reference and `size` payload bytes.
pub(crate) unsafe fn object(kind: u32, size: usize) -> Word {
    unsafe {
        let pointer = alloc(layout(size)).cast::<Header>();
        if pointer.is_null() {
            std::process::abort();
        }
        LIVE.set(LIVE.get() + 1);
        pointer.write(Header {
            count: 1,
            kind,
            size: size as u64,
        });
        pointer as usize as Word
    }
}

pub(crate) fn small(value: i64) -> Option<Word> {
    (SMALL_MIN..=SMALL_MAX)
        .contains(&value)
        .then_some(((value << 1) | 1) as Word)
}

pub(crate) fn boolean(value: bool) -> Word {
    if value { TRUE } else { FALSE }
}

/// The integer a word holds. The word must be an integer.
pub(crate) unsafe fn int(word: Word) -> i64 {
    if word & 1 == 1 {
        (word as i64) >> 1
    } else {
        unsafe { payload(word).cast::<i64>().read() }
    }
}

pub(crate) unsafe fn float(word: Word) -> f64 {
    unsafe { f64::from_bits(payload(word).read()) }
}

pub(crate) fn fault(context: *mut Context, fault: Fault) -> Word {
    unsafe {
        (*context).fault = fault as u64;
    }
    UNWIND
}

// Entry points

/// Takes one more reference to a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_retain(word: Word) {
    if is_object(word) {
        unsafe {
            let header = header(word);
            if (*header).count != PINNED {
                (*header).count += 1;
            }
        }
    }
}

/// Gives up one reference to a value, reclaiming it if that was the last.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_release(word: Word) {
    // A value can be nested arbitrarily deep, so the values an object held
    // are queued here, never released by recursion.
    let mut pending = vec![word];
    while let Some(word) = pending.pop() {
        if !is_object(word) {
            continue;
        }
        unsafe {
            let header = header(word);
            if (*header).count == PINNED {
                continue;
            }
            (*header).count -= 1;
            if (*header).count != 0 {
                continue;
            }
            let held = match (*header).kind {
                KIND_TUPLE | KIND_LIST | KIND_RECORD => words(word),
                // After the value and the type arguments it was made
                // under is the type the cell was made at.
                KIND_CELL => {
                    crate::cell::unlink(word);
                    &words(word)[..2]
                }
                // The first word is the definition, which is not a value.
                KIND_CLOSURE => &words(word)[1..],
                // Each argument is a node, which is not a value, and the
                // type arguments it is read in.
                KIND_SCOPE => {
                    pending.extend(words(word).iter().skip(1).step_by(2));
                    &[]
                }
                _ => &[],
            };
            pending.extend_from_slice(held);
            let size = (*header).size as usize;
            dealloc(header.cast(), layout(size));
            LIVE.set(LIVE.get() - 1);
        }
    }
}

/// The payload of an object, as words.
pub(crate) unsafe fn words<'a>(word: Word) -> &'a mut [u64] {
    unsafe {
        let length = (*header(word)).size as usize / 8;
        std::slice::from_raw_parts_mut(payload(word), length)
    }
}

pub(crate) unsafe fn kind(word: Word) -> u32 {
    if is_object(word) {
        unsafe { (*header(word)).kind }
    } else {
        0
    }
}

pub(crate) unsafe fn bytes<'a>(word: Word) -> &'a [u8] {
    unsafe {
        let length = (*header(word)).size as usize;
        std::slice::from_raw_parts(payload(word).cast::<u8>(), length)
    }
}
