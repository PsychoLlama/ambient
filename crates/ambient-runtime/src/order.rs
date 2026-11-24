//! The order of values: see "Order" in `docs/spec/operations.md`.

use core::cmp::Ordering;

use crate::object::{KIND_BYTES, KIND_INT, KIND_STRING, KIND_TAG, bytes, header, int, is_object};
use crate::{FALSE, NULL, TRUE, Word};

/// Whether `left` comes before `right`, is the same, or comes after
/// it, as -1, 0, or 1. Checking holds both to the shapes order takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_order(left: Word, right: Word) -> Word {
    let ordering = unsafe { order(left, right) };
    unsafe { crate::amb_int_new(ordering as i64) }
}

unsafe fn order(left: Word, right: Word) -> Ordering {
    if left == right {
        return Ordering::Equal;
    }
    let (rank_left, rank_right) = unsafe { (rank(left), rank(right)) };
    if rank_left != rank_right {
        return rank_left.cmp(&rank_right);
    }
    unsafe {
        match rank_left {
            Rank::Bool => (left == TRUE).cmp(&(right == TRUE)),
            Rank::Int => int(left).cmp(&int(right)),
            Rank::String | Rank::Bytes | Rank::Tag => bytes(left).cmp(bytes(right)),
            // Null is one value, and a shape order does not take was
            // ruled out by checking: nothing tells two of it apart.
            Rank::Null | Rank::Other => Ordering::Equal,
        }
    }
}

/// The shapes order takes, in their order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Null,
    Bool,
    Int,
    String,
    Bytes,
    Tag,
    Other,
}

unsafe fn rank(word: Word) -> Rank {
    match word {
        NULL => return Rank::Null,
        TRUE | FALSE => return Rank::Bool,
        _ => {}
    }
    if word & 1 == 1 {
        return Rank::Int;
    }
    if !is_object(word) {
        return Rank::Other;
    }
    match unsafe { (*header(word)).kind } {
        KIND_INT => Rank::Int,
        KIND_STRING => Rank::String,
        KIND_BYTES => Rank::Bytes,
        KIND_TAG => Rank::Tag,
        _ => Rank::Other,
    }
}
