//! Lists. See "Sharing" in `docs/spec/values.md`.
//!
//! A list is a view: a buffer, where in it the list starts, and how
//! many members it has. A buffer has room for more members than it
//! holds, and says how many it holds. A list that ends where its buffer
//! does is the newest over that buffer, and pushing onto it writes the
//! member into the room after it; the lists made before see only their
//! own stretch, which nothing writes to again. So a push adds one
//! member, a slice is another view, and no list ever changes.
//!
//! Every word of a list and of a buffer is a value, so each is
//! reclaimed like a tuple: room that holds nothing holds null.

use crate::object::{KIND_BUFFER, KIND_LIST, amb_retain, int, kind, object, small, words};
use crate::sequence::range;
use crate::{NULL, Word, amb_int_new};

/// The words of a list: its buffer, or null if it has no members, then
/// where it starts in the buffer and how many members it has.
const BUFFER: usize = 0;
const START: usize = 1;
const LENGTH: usize = 2;

/// The least room a buffer that is grown into has.
const LEAST: usize = 4;

/// A count as a word. One that an address can reach is immediate.
fn count(value: usize) -> Word {
    small(value as i64).unwrap_or(NULL)
}

/// A buffer with room for `room` members, that holds none.
unsafe fn buffer(room: usize) -> Word {
    unsafe {
        let word = object(KIND_BUFFER, (room + 1) * 8);
        words(word).fill(NULL);
        words(word)[0] = count(0);
        word
    }
}

/// How many members a buffer holds, and the room it has for them.
unsafe fn held<'a>(buffer: Word) -> (usize, &'a mut [Word]) {
    unsafe {
        let (held, room) = words(buffer).split_at_mut(1);
        (int(held[0]) as usize, room)
    }
}

/// A list of `length` members of `buffer` from `start`. It takes the
/// reference to the buffer.
unsafe fn view(buffer: Word, start: usize, length: usize) -> Word {
    unsafe {
        let word = object(KIND_LIST, 24);
        words(word).copy_from_slice(&[buffer, count(start), count(length)]);
        word
    }
}

/// Where a list is: its buffer, its start, and its length.
unsafe fn place(list: Word) -> (Word, usize, usize) {
    unsafe {
        let words = words(list);
        (
            words[BUFFER],
            int(words[START]) as usize,
            int(words[LENGTH]) as usize,
        )
    }
}

/// The members of a list, or of a tuple.
pub(crate) unsafe fn members<'a>(sequence: Word) -> &'a [Word] {
    unsafe { members_mut(sequence) }
}

/// The members of a list or a tuple that is under construction, to
/// fill in.
pub(crate) unsafe fn members_mut<'a>(sequence: Word) -> &'a mut [Word] {
    unsafe {
        if kind(sequence) != KIND_LIST {
            return words(sequence);
        }
        let (buffer, start, length) = place(sequence);
        if length == 0 {
            return &mut [];
        }
        &mut held(buffer).1[start..start + length]
    }
}

/// A list of `length` nulls, to fill in before anything else sees it.
pub(crate) unsafe fn filled(length: usize) -> Word {
    unsafe {
        if length == 0 {
            return view(NULL, 0, 0);
        }
        let buffer = buffer(length);
        words(buffer)[0] = count(length);
        view(buffer, 0, length)
    }
}

/// A list of the members of `list`, then `more`.
///
/// If `list` is the newest over its buffer and the buffer has room,
/// `more` goes into that room. If not, the members are copied to a
/// buffer with room to spare, so that the pushes that follow do not
/// copy again.
unsafe fn extended(list: Word, more: &[Word]) -> Word {
    unsafe {
        let (old, start, length) = place(list);
        let total = length + more.len();
        let fits = length != 0 && {
            let (held, room) = held(old);
            start + length == held && start + total <= room.len()
        };
        let (buffer, start) = if fits {
            amb_retain(old);
            (old, start)
        } else {
            let new = buffer((total + total / 2).max(LEAST));
            for (slot, member) in held(new).1.iter_mut().zip(members(list)) {
                amb_retain(*member);
                *slot = *member;
            }
            (new, 0)
        };
        for (slot, member) in held(buffer).1[start + length..].iter_mut().zip(more) {
            amb_retain(*member);
            *slot = *member;
        }
        words(buffer)[0] = count(start + total);
        view(buffer, start, total)
    }
}

/// How many members a list, or a tuple used as one, has.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_length(value: Word) -> Word {
    unsafe { amb_int_new(members(value).len() as i64) }
}

/// The member at a position, or null if there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_get(value: Word, position: Word) -> Word {
    unsafe {
        let position = usize::try_from(int(position)).ok();
        match position.and_then(|position| members(value).get(position)) {
            Some(member) => {
                amb_retain(*member);
                *member
            }
            None => NULL,
        }
    }
}

/// A list of the members of one, then `member`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_push(value: Word, member: Word) -> Word {
    unsafe { extended(value, &[member]) }
}

/// A list of the members of one, then those of another.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_join(left: Word, right: Word) -> Word {
    unsafe { extended(left, members(right)) }
}

/// The members between two positions, or null if the positions are out
/// of range or out of order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_list_slice(value: Word, start: Word, end: Word) -> Word {
    unsafe {
        let (buffer, from, length) = place(value);
        match range(start, end, length) {
            Some(range) if range.is_empty() => view(NULL, 0, 0),
            Some(range) => {
                amb_retain(buffer);
                view(buffer, from + range.start, range.len())
            }
            None => NULL,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use proptest::prelude::*;

    use super::*;
    use crate::object::amb_release;

    /// Something done to one of the lists made so far, by position.
    #[derive(Clone, Debug)]
    enum Step {
        Push(usize, i64),
        Join(usize, usize),
        Slice(usize, usize, usize),
    }

    fn step() -> impl Strategy<Value = Step> {
        let at = 0usize..64;
        prop_oneof![
            (at.clone(), any::<i64>()).prop_map(|(list, member)| Step::Push(list, member)),
            (at.clone(), at.clone()).prop_map(|(left, right)| Step::Join(left, right)),
            (at.clone(), at.clone(), at)
                .prop_map(|(list, start, end)| Step::Slice(list, start, end)),
        ]
    }

    fn ints(list: Word) -> Vec<i64> {
        // SAFETY: the word is a list of integers.
        unsafe { members(list).iter().map(|member| int(*member)).collect() }
    }

    /// Takes `step` on lists and on vectors alike. Returns the list it
    /// made, if it made one.
    fn take(step: &Step, lists: &[Word], model: &[Vec<i64>]) -> Option<(Word, Vec<i64>)> {
        let pick = |index: &usize| index % lists.len();
        // SAFETY: every word is a list this test made, or an integer.
        unsafe {
            match step {
                Step::Push(list, member) => {
                    let list = pick(list);
                    let word = amb_int_new(*member);
                    let pushed = amb_list_push(lists[list], word);
                    amb_release(word);
                    Some((pushed, [model[list].as_slice(), &[*member]].concat()))
                }
                Step::Join(left, right) => {
                    let (left, right) = (pick(left), pick(right));
                    let joined = amb_list_join(lists[left], lists[right]);
                    Some((joined, [model[left].as_slice(), &model[right]].concat()))
                }
                Step::Slice(list, start, end) => {
                    let list = pick(list);
                    let (from, to) = (amb_int_new(*start as i64), amb_int_new(*end as i64));
                    let sliced = amb_list_slice(lists[list], from, to);
                    let expected = model[list].get(*start..*end);
                    assert_eq!(sliced == NULL, expected.is_none());
                    Some((sliced, expected?.to_vec()))
                }
            }
        }
    }

    proptest! {
        #[test]
        fn lists_are_what_vectors_would_be_and_no_list_changes(
            steps in prop::collection::vec(step(), 0..64),
        ) {
            let live = crate::object::live();
            // SAFETY: making a list has no preconditions.
            let mut lists = alloc::vec![unsafe { filled(0) }];
            let mut model = alloc::vec![Vec::new()];
            for step in &steps {
                if let Some((list, expected)) = take(step, &lists, &model) {
                    lists.push(list);
                    model.push(expected);
                }
                // Every list made so far is still what it was.
                for (list, expected) in lists.iter().zip(&model) {
                    prop_assert_eq!(&ints(*list), expected);
                }
            }
            for list in lists {
                // SAFETY: the test owns each list it made.
                unsafe { amb_release(list) };
            }
            prop_assert_eq!(crate::object::live(), live);
        }
    }

    #[test]
    fn a_push_onto_the_newest_list_copies_nothing() {
        // SAFETY: every word is a list this test made, or immediate.
        unsafe {
            let first = amb_list_push(filled(0), amb_int_new(1));

            let second = amb_list_push(first, amb_int_new(2));

            assert_eq!(members(first).as_ptr(), members(second).as_ptr());
            assert_eq!(
                (ints(first), ints(second)),
                (alloc::vec![1], alloc::vec![1, 2])
            );
        }
    }

    #[test]
    fn a_second_push_onto_one_list_leaves_the_first_alone() {
        // SAFETY: every word is a list this test made, or immediate.
        unsafe {
            let base = amb_list_push(filled(0), amb_int_new(1));
            let first = amb_list_push(base, amb_int_new(2));

            let second = amb_list_push(base, amb_int_new(3));

            assert_eq!(
                (ints(first), ints(second)),
                (alloc::vec![1, 2], alloc::vec![1, 3])
            );
        }
    }

    #[test]
    fn a_slice_is_a_view_of_the_list_it_was_cut_from() {
        // SAFETY: every word is a list this test made, or immediate.
        unsafe {
            let whole = amb_list_join(filled(0), amb_list_push(filled(0), amb_int_new(7)));
            let whole = amb_list_push(whole, amb_int_new(8));

            let rest = amb_list_slice(whole, amb_int_new(1), amb_int_new(2));

            assert_eq!(members(rest).as_ptr(), members(whole)[1..].as_ptr());
        }
    }
}
