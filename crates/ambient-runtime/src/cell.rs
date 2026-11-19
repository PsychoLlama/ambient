//! Cells: the only mutable thing. See `docs/spec/cells.md`.

use crate::number::equal;
use crate::object::{KIND_CELL, amb_release, amb_retain, boolean, fault, object, words};
use crate::scope::{At, Env};
use crate::task::{amb_suspend, arriving, interrupted, wait_on, waiting, wake};
use crate::unit::{self, amb_unit_retain};
use crate::{Context, Fault, NULL, Word};

/// A cell that holds `value`, made at the type `ty` describes, a
/// descriptor's address, read in `scope`: the type arguments of the
/// function that makes it, or null. The cell keeps both, which is what
/// lets a cast tell what the cell may hold, and holds the descriptor's
/// unit.
///
/// If the host keeps the program's cells, the cell joins them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_new(
    context: *mut Context,
    value: Word,
    ty: u64,
    scope: Word,
) -> Word {
    unsafe {
        let word = object(KIND_CELL, 48);
        amb_retain(value);
        amb_retain(scope);
        amb_unit_retain(unit::of(ty));
        let fields = words(word);
        (fields[0], fields[1], fields[2]) = (value, scope, ty);
        (fields[BACK], fields[NEXT], fields[WAITED]) = (0, 0, 0);
        let first = (*context).cells as usize as *mut u64;
        if !first.is_null() {
            link(first, word);
        }
        word
    }
}

/// Where a cell keeps its place among its program's live cells: the
/// address of the word that refers to it, which is the host's or the
/// cell before's, and the cell after it. Neither is counted. Both are
/// zero in a cell that is among none.
const BACK: usize = 3;
const NEXT: usize = 4;

/// Where a cell keeps which task has waited longest for it to change:
/// its number and one, or zero if none waits. The scheduler keeps the
/// rest of them (`task/watch.rs`).
const WAITED: usize = 5;

/// The word of a cell that says which task has waited longest on it.
pub(crate) unsafe fn waited(cell: Word) -> *mut u64 {
    unsafe { &raw mut words(cell)[WAITED] }
}

/// Puts a cell first among the cells `first` begins.
unsafe fn link(first: *mut u64, cell: Word) {
    unsafe {
        let next = *first;
        let fields = words(cell);
        (fields[BACK], fields[NEXT]) = (first as u64, next);
        if next != 0 {
            words(next)[BACK] = (&raw mut fields[NEXT]) as u64;
        }
        *first = cell;
    }
}

/// Takes a cell from among its program's live cells, if it is among
/// them: it is being reclaimed, or emptied.
pub(crate) unsafe fn unlink(cell: Word) {
    unsafe {
        let fields = words(cell);
        let (back, next) = (fields[BACK] as usize as *mut u64, fields[NEXT]);
        if back.is_null() {
            return;
        }
        *back = next;
        if next != 0 {
            words(next)[BACK] = back as u64;
        }
        (fields[BACK], fields[NEXT]) = (0, 0);
    }
}

/// Empties every live cell of a program, which is over: `first` is the
/// word its contexts were given in [`Context::cells`]. Counting alone
/// never reclaims cells that reach themselves; this is what does, once
/// nothing can read them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cells_drop(first: *mut u64) {
    unsafe {
        loop {
            let cell = *first;
            if cell == 0 {
                break;
            }
            unlink(cell);
            // Releasing what it held may reclaim other cells, which
            // take themselves from the list.
            amb_release(core::mem::replace(&mut words(cell)[0], NULL));
        }
    }
}

/// The type a cell was made at.
pub(crate) unsafe fn made_at(cell: Word) -> At {
    unsafe { (words(cell)[2], Env::given(words(cell)[1])) }
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
        amb_release(core::mem::replace(slot, new));
        wake(context, cell);
        boolean(true)
    }
}

/// Suspends the task until a cell holds something other than `seen`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cell_wait(context: *mut Context, cell: Word, seen: Word) -> Word {
    unsafe { waiting(context, || changed(context, cell, seen)) }
}

/// A step of a wait on a cell: asked again each time the cell changes.
unsafe fn changed(context: *mut Context, cell: Word, seen: Word) -> Word {
    unsafe {
        // Arriving or not, a cancelled task unwinds from here.
        arriving(context);
        wait_on(context, None);
        if interrupted(context) {
            return fault(context, Fault::Cancelled);
        }
        if !equal(words(cell)[0], seen) {
            return NULL;
        }
        wait_on(context, Some(cell));
        amb_suspend(context)
    }
}
