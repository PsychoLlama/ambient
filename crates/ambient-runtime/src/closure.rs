//! Closures, and starting a fault.

use crate::object::layout::{DEFINITION_HASH, DEFINITION_TYPE};
use crate::object::{KIND_CLOSURE, amb_retain, bytes, fault, object, words};
use crate::unit::{self, amb_unit_retain};
use crate::{Context, Fault, NULL, Word};

/// A closure over `definition` with room for `captures` values, which
/// holds the definition's unit. Fill it
/// with [`amb_closure_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_new(definition: u64, captures: u64) -> Word {
    unsafe {
        let word = object(KIND_CLOSURE, (captures as usize + 1) * 8);
        let fields = words(word);
        fields.fill(NULL);
        fields[0] = definition;
        amb_unit_retain(unit::of(definition));
        word
    }
}

/// Puts `value` in capture `index` of a closure under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_set(closure: Word, index: u64, value: Word) {
    unsafe {
        amb_retain(value);
        words(closure)[index as usize + 1] = value;
    }
}

/// Whether two definitions are one: the same, or loaded apart with one
/// hash.
pub(crate) unsafe fn same_definition(left: u64, right: u64) -> bool {
    let hash = |definition: u64| unsafe {
        let definition = definition as usize as *const u64;
        bytes(definition.add(DEFINITION_HASH as usize / 8).read())
    };
    left == right || hash(left) == hash(right)
}

/// The name of the type of a closure's function, or zero: see
/// [`DEFINITION_TYPE`].
pub(crate) unsafe fn type_name(closure: Word) -> u64 {
    unsafe {
        let definition = words(closure)[0] as usize as *const u64;
        definition.add(DEFINITION_TYPE as usize / 8).read()
    }
}

/// Starts unwinding with a fault. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_fault(context: *mut Context, code: u64) -> Word {
    fault(context, Fault::from_code(code))
}
