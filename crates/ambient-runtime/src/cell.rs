//! Cells: the only mutable thing. See `spec/cells.md`.

use crate::number::equal;
use crate::object::{KIND_CELL, amb_release, amb_retain, boolean, fault, object, words};
use crate::task::{arriving, cancelled, suspend, wait_on, wake};
use crate::{Context, Fault, NULL, Word};

/// A cell that holds `value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_new(value: Word) -> Word {
    unsafe {
        let word = object(KIND_CELL, 8);
        amb_retain(value);
        words(word)[0] = value;
        word
    }
}

/// The value a cell holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_read(cell: Word) -> Word {
    unsafe {
        let value = words(cell)[0];
        amb_retain(value);
        value
    }
}

/// Puts `new` in a cell if it still holds `seen`, and says whether it
/// did. Tasks waiting for the cell to change get their turn.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_replace(
    context: *mut Context,
    cell: Word,
    seen: Word,
    new: Word,
) -> Word {
    unsafe {
        let slot = &mut words(cell)[0];
        if !equal(*slot, seen) {
            return boolean(false);
        }
        amb_retain(new);
        amb_release(std::mem::replace(slot, new));
        wake(context, cell);
        boolean(true)
    }
}

/// Suspends the task until a cell holds something other than `seen`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_wait(context: *mut Context, cell: Word, seen: Word) -> Word {
    unsafe {
        if arriving(context) && cancelled(context) {
            return fault(context, Fault::Cancelled);
        }
        if !equal(words(cell)[0], seen) {
            return NULL;
        }
        wait_on(context, Some(cell));
        suspend(context)
    }
}
