//! Closures, and starting a fault.

use crate::object::layout::DEFINITION_TYPE;
use crate::object::{KIND_CLOSURE, amb_retain, fault, object, words};
use crate::{Context, Fault, NULL, Word};

/// A closure over `definition` with room for `captures` values. Fill it
/// with [`amb_closure_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_new(definition: u64, captures: u64) -> Word {
    unsafe {
        let word = object(KIND_CLOSURE, (captures as usize + 1) * 8);
        let fields = words(word);
        fields.fill(NULL);
        fields[0] = definition;
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
