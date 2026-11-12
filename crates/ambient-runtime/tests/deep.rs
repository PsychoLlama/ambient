//! Values nested deeper than any stack. Nothing that walks a value may
//! recurse as deep as the value nests.

use ambient_runtime::{
    FALSE, NULL, TRUE, Word, amb_equal, amb_is, amb_live, amb_release, amb_sequence_new,
    amb_sequence_set, descriptor,
};

const DEPTH: usize = 1_000_000;

/// A tuple holding `inner` alone, which it takes over.
fn around(inner: Word) -> Word {
    unsafe {
        let tuple = amb_sequence_new(0, 1);
        amb_sequence_set(tuple, 0, inner);
        amb_release(inner);
        tuple
    }
}

/// `innermost` inside [`DEPTH`] tuples.
fn nested(innermost: Word) -> Word {
    (0..DEPTH).fold(innermost, |inner, _| around(inner))
}

/// The descriptor of `T = null | (T,)`, which follows a value all the
/// way down. Never reclaimed, as descriptors are not.
fn nesting() -> u64 {
    let nodes: &'static mut [u64] = Box::leak(Box::new([0; 8]));
    let address = |nodes: &[u64], index: usize| std::ptr::from_ref(&nodes[index]) as u64;
    let (null, tuple) = (address(nodes, 4), address(nodes, 5));
    let union = address(nodes, 0);
    nodes.copy_from_slice(&[
        descriptor::UNION,
        2,
        null,
        tuple,
        descriptor::NULL,
        descriptor::TUPLE,
        1,
        union,
    ]);
    union
}

#[test]
fn deep_values_are_compared_without_recursion() {
    let before = amb_live();
    let (left, right, other) = (nested(NULL), nested(NULL), nested(TRUE));

    unsafe {
        assert_eq!(amb_equal(left, right), TRUE);
        assert_eq!(amb_equal(left, other), FALSE);
        for value in [left, right, other] {
            amb_release(value);
        }
    }
    assert_eq!(amb_live(), before);
}

#[test]
fn a_type_follows_a_deep_value_without_recursion() {
    let (fits, differs) = (nested(NULL), nested(TRUE));

    unsafe {
        assert_eq!(amb_is(fits, nesting(), NULL), TRUE);
        assert_eq!(amb_is(differs, nesting(), NULL), FALSE);
        for value in [fits, differs] {
            amb_release(value);
        }
    }
}
