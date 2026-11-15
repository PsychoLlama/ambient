//! Handles: opaque references native code mints. See `spec/values.md`.
//!
//! A handle is the brand it was minted under, as a tag, and a number
//! its host knows it by. Nothing in a program reads either; the host
//! is told when the last reference to one goes, so that what it stands
//! for can be closed.

use core::cell::Cell;

use crate::Word;
use crate::object::{KIND_HANDLE, free, object, payload};

/// Where a handle holds the next of those dropped after it.
const NEXT: usize = 2;

/// The handles nothing refers to any more, which the host has not yet
/// asked after: the oldest and the newest, each holding the one after
/// it, and the number of the one the host was last given. They are
/// kept, not copied, so a program that never has a handle carries
/// nothing to keep them in.
struct Dropped {
    first: Cell<Word>,
    last: Cell<Word>,
    number: Cell<u64>,
}

impl Dropped {
    const fn new() -> Self {
        Self {
            first: Cell::new(0),
            last: Cell::new(0),
            number: Cell::new(0),
        }
    }
}

// A thread runs one program at a time, and a host asks after what was
// dropped before it runs another.
#[cfg(not(target_arch = "wasm32"))]
std::thread_local! {
    static DROPPED: Dropped = const { Dropped::new() };
}

#[cfg(not(target_arch = "wasm32"))]
fn with<R>(work: impl FnOnce(&Dropped) -> R) -> R {
    DROPPED.with(work)
}

/// The same, of a program that is alone in its memory.
#[cfg(target_arch = "wasm32")]
struct Alone(Dropped);

// There is one thread.
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for Alone {}

#[cfg(target_arch = "wasm32")]
#[unsafe(export_name = "amb_state_dropped")]
static DROPPED: Alone = Alone(Dropped::new());

#[cfg(target_arch = "wasm32")]
fn with<R>(work: impl FnOnce(&Dropped) -> R) -> R {
    work(&DROPPED.0)
}

/// A handle minted under `brand`, a tag, that its host knows by
/// `number`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_handle_new(brand: Word, number: u64) -> Word {
    unsafe {
        let word = object(KIND_HANDLE, 24);
        let held = payload(word);
        held.write(brand);
        held.add(1).write(number);
        held.add(NEXT).write(0);
        word
    }
}

/// Keeps a handle nothing refers to for its host to hear of, in place
/// of reclaiming it.
pub(crate) unsafe fn dropped(handle: Word) {
    with(|dropped| unsafe {
        match dropped.last.replace(handle) {
            0 => dropped.first.set(handle),
            last => payload(last).add(NEXT).write(handle),
        }
    });
}

/// The brand of the next handle that was dropped, a tag, or zero if
/// the host has heard of every one. [`amb_handle_number`] then gives
/// its number.
#[unsafe(no_mangle)]
pub extern "C" fn amb_handle_next() -> Word {
    with(|dropped| {
        let handle = dropped.first.get();
        if handle == 0 {
            return 0;
        }
        // SAFETY: only handles are kept, each until it is taken here.
        unsafe {
            let held = payload(handle);
            let (brand, number, next) = (held.read(), held.add(1).read(), held.add(NEXT).read());
            dropped.first.set(next);
            if next == 0 {
                dropped.last.set(0);
            }
            dropped.number.set(number);
            free(handle);
            brand
        }
    })
}

/// The number of the handle [`amb_handle_next`] last gave.
#[unsafe(no_mangle)]
pub extern "C" fn amb_handle_number() -> u64 {
    with(|dropped| dropped.number.get())
}
