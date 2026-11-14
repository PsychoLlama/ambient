//! Closures, and starting a fault.

use crate::object::layout::{
    DEFINITION_DESCRIBED, DEFINITION_HASH, DEFINITION_TYPE, DEFINITION_TYPES,
};
use crate::object::{KIND_CLOSURE, KIND_SCOPE, amb_retain, bytes, fault, kind, object, words};
use crate::scope::{At, Env};
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

/// How many type arguments a closure's function is handed as it runs.
unsafe fn handed(closure: Word) -> usize {
    unsafe {
        let definition = words(closure)[0] as usize as *const u64;
        definition.add(DEFINITION_TYPES as usize / 8).read() as usize
    }
}

/// The type arguments a closure holds, which follow what it captured,
/// if it holds any.
unsafe fn scope(closure: Word) -> Option<Word> {
    unsafe {
        let last = *words(closure)[1..].last()?;
        (kind(last) == KIND_SCOPE).then_some(last)
    }
}

/// Whether a closure is still a generic function: its function needs
/// no type arguments, or it does not hold them all yet.
pub(crate) unsafe fn generic(closure: Word) -> bool {
    unsafe {
        let held = scope(closure).map_or(0, |scope| words(scope).len() / 2);
        held < handed(closure) || handed(closure) == 0
    }
}

/// The type of a closure's function where that depends on type
/// arguments: a node, read in those the closure holds. Nothing if the
/// type does not depend on them, or the closure does not hold them all.
pub(crate) unsafe fn described(closure: Word) -> Option<At> {
    unsafe {
        let definition = words(closure)[0] as usize as *const u64;
        let node = definition.add(DEFINITION_DESCRIBED as usize / 8).read();
        let scope = scope(closure)?;
        (node != 0 && !generic(closure)).then_some((node, Env::given(scope)))
    }
}

/// A generic function at type arguments: `types`, which follow those
/// the closure already holds. A function that is handed none is one
/// function at every type, and is given back as it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_instantiate(closure: Word, types: Word) -> Word {
    unsafe {
        if handed(closure) == 0 {
            amb_retain(closure);
            return closure;
        }
        let held = scope(closure);
        let fields = words(closure);
        let captures = &fields[1..fields.len() - usize::from(held.is_some())];
        let made = object(kind(closure), (captures.len() + 2) * 8);
        let filled = words(made);
        filled[0] = fields[0];
        amb_unit_retain(unit::of(fields[0]));
        for (slot, capture) in filled[1..].iter_mut().zip(captures) {
            amb_retain(*capture);
            *slot = *capture;
        }
        filled[captures.len() + 1] = crate::scope::joined(held, types);
        made
    }
}

/// Starts unwinding with a fault. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_fault(context: *mut Context, code: u64) -> Word {
    fault(context, Fault::from_code(code))
}
