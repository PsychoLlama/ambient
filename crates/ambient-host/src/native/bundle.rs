//! A native bundle's side of a call: what its functions are handed and
//! what they call back through. See "Native bundles" in
//! `docs/spec/externs.md`.
//!
//! A function of a native bundle reads its arguments where they lie,
//! through the runtime's own entry points, and makes what it gives the
//! same way. What only a host knows of the call it is in (who waits,
//! what its row bound, how it failed) it asks through the entry points
//! here, each of which takes the call it was handed.

use std::borrow::Cow;
use std::slice;

use ambient_native::Dropped;
use ambient_runtime::{Kind, UNWIND, Word};

use super::host::{ANSWERING, Call, Running, UNCALLABLE, UNWOUND, bound};

/// The call a function of a native bundle is in, as it is handed it:
/// nothing of it is the function's to read.
pub use ambient_native::Call as Calling;

/// A function of a native bundle: the call it is in and its arguments,
/// a tuple it does not own, in; a value its caller owns out.
///
/// One that failed ([`amb_bundle_fail`]), that answers later
/// ([`amb_bundle_later`]), or whose call back unwound gives null, which
/// is not looked at.
pub type Bundled = unsafe extern "C" fn(call: *mut Calling, args: Word) -> Word;

/// What a host that copies values says of a native bundle's function.
pub const PLACED: &str =
    "a native bundle's function reads values where they lie, which only a native program lets it";

/// Runs `function` for `call` with `args`, and gives what it gave, or
/// what it said went wrong.
///
/// # Safety
///
/// `function` is what a native bundle has for the extern `call` is of,
/// and `args` is the tuple the runtime handed over.
pub(super) unsafe fn run(function: Bundled, call: &mut Call, args: Word) -> Result<Word, String> {
    let handed: *mut Call = call;
    // SAFETY: the call outlives the function's run, and the function
    // reaches it only through the entry points below.
    let gave = unsafe { function(handed.cast(), args) };
    call.failed.take().map_or(Ok(gave), Err)
}

/// The call behind what a function was handed.
///
/// # Safety
///
/// `call` is what the function running was handed, and that function
/// has not returned.
unsafe fn behind<'a>(call: *mut Calling) -> &'a mut Call<'a> {
    // SAFETY: `run` made the pointer of a call that is still live.
    unsafe { &mut *call.cast() }
}

/// The text native code handed over.
///
/// # Safety
///
/// `text` is `length` bytes.
unsafe fn text<'a>(text: *const u8, length: u64) -> Cow<'a, str> {
    let length = usize::try_from(length).unwrap_or(0);
    // SAFETY: the caller's to uphold.
    String::from_utf8_lossy(unsafe { slice::from_raw_parts(text, length) })
}

/// Says the call failed, with a message of `length` bytes: the task
/// that made it faults, as it does when any native code fails.
///
/// # Safety
///
/// `call` is what the function running was handed, and `message` is
/// `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_fail(call: *mut Calling, message: *const u8, length: u64) {
    // SAFETY: the caller's to uphold.
    unsafe { behind(call).failed = Some(text(message, length).into_owned()) }
}

/// Says the call will answer later, and gives the ticket its answer is
/// to come by: what [`super::amb_bundle_answer`] takes. Zero, with the call failed, if the extern was not
/// declared `suspending`.
///
/// # Safety
///
/// `call` is what the function running was handed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_later(call: *mut Calling) -> u64 {
    // SAFETY: the caller's to uphold.
    let call = unsafe { behind(call) };
    match call.later() {
        // A thread answers by a number no other program has.
        Ok(ticket) => call.program.posts.name(ticket),
        Err(refused) => {
            call.failed = Some(refused);
            0
        }
    }
}

/// Has `dropped` called with the number of each handle of `brand`
/// that the program holds no longer, as a host is told of its own
/// ([`super::Waits::dropped`]) and at the same moments.
///
/// # Safety
///
/// `call` is what the function running was handed, `brand` is a tag,
/// and `dropped` may be called for as long as the program is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_dropping(call: *mut Calling, brand: Word, dropped: Dropped) {
    // SAFETY: the caller's to uphold; a tag's bytes are read for as
    // long as it is held, which the call does.
    unsafe {
        let call = behind(call);
        let length = usize::try_from(ambient_runtime::amb_length(brand)).unwrap_or(0);
        let text = slice::from_raw_parts(ambient_runtime::amb_string_bytes(brand), length);
        call.program
            .dropping
            .borrow_mut()
            .insert(text.to_vec(), dropped);
    }
}

/// The handler bound for an ability of the extern's row, named by its
/// package and its module path and name: a function the call holds,
/// to call with [`amb_bundle_call`]. Zero if the row has none such.
///
/// # Safety
///
/// `call` is what the function running was handed, and each text is as
/// long as the length after it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_handler(
    call: *mut Calling,
    package: *const u8,
    package_length: u64,
    path: *const u8,
    path_length: u64,
) -> Word {
    // SAFETY: the caller's to uphold; the place is within the tuple of
    // handlers, which the extern's caller holds for the call.
    unsafe {
        let call = behind(call);
        let (package, path) = (text(package, package_length), text(path, path_length));
        bound(&call.row, &package, &path)
            .map_or(UNWIND, |at| ambient_runtime::amb_word_at(call.handlers, at))
    }
}

/// Calls a function value of the program with `count` arguments and
/// nothing bound, and gives what it gave, which the caller owns. The
/// call is the task's that called the native code.
///
/// Zero if the call unwound, or could not be made: the function that
/// asked is then over, and gives null at once.
///
/// # Safety
///
/// `call` is what the function running was handed, `function` is a
/// value, and `args` is `count` values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_call(
    call: *mut Calling,
    function: Word,
    args: *const Word,
    count: u64,
) -> Word {
    // SAFETY: the caller's to uphold.
    let call = unsafe { behind(call) };
    let over = [(call.unwound, UNWOUND), (call.ticket.is_some(), ANSWERING)];
    if let Some((_, refused)) = over.iter().find(|(is, _)| *is) {
        call.failed = Some((*refused).to_owned());
        return UNWIND;
    }
    // SAFETY: a value's kind can always be asked, and only a function
    // is asked its arity. Each index is within the length asked for,
    // and a tuple keeps a reference of its own to what it is set.
    unsafe {
        let kind = ambient_runtime::amb_kind(function);
        let callable = kind == Kind::Function as u64 || kind == Kind::Method as u64;
        if !callable || ambient_runtime::amb_closure_arity(function) != count + 1 {
            call.failed = Some(UNCALLABLE.to_owned());
            return UNWIND;
        }
        let handed = ambient_runtime::amb_sequence_new(0, count + 1);
        let args = slice::from_raw_parts(args, usize::try_from(count).unwrap_or(0));
        for (index, arg) in (1..).zip(args) {
            ambient_runtime::amb_sequence_set(handed, index, *arg);
        }
        let gave = ambient_runtime::amb_call_among(call.context, function, handed);
        ambient_runtime::amb_release(handed);
        call.unwound = gave == UNWIND;
        gave
    }
}

#[cfg(test)]
mod tests {
    use ambient_runtime::{Kind, Word};

    /// Each entry a bundle is told of is a function here, of the
    /// signature it is told.
    macro_rules! checked {
        ($($(#[doc = $about:literal])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $gives:ty)?;)*) => {
            #[test]
            fn each_entry_a_bundle_is_told_of_is_as_it_is_told() {
                #[allow(clippy::wildcard_imports)]
                use ambient_runtime::*;
                #[allow(clippy::wildcard_imports)]
                use super::*;
                #[allow(clippy::wildcard_imports)]
                use crate::native::posts::*;
                use ambient_native::{Data, Dropped, Make};
                type Call = Calling;
                $(let _: unsafe extern "C" fn($($ty),*) $(-> $gives)? = $name;)*
            }
        };
    }
    ambient_native::entries!(checked);

    #[test]
    fn what_a_bundle_is_told_of_values_is_what_the_runtime_holds() {
        use ambient_native::kind;
        let words = [
            ambient_native::NULL,
            ambient_native::FALSE,
            ambient_native::TRUE,
        ];
        assert_eq!(
            words,
            [
                ambient_runtime::NULL,
                ambient_runtime::FALSE,
                ambient_runtime::TRUE
            ]
        );
        let kinds: [(u64, Kind); 12] = [
            (kind::NULL, Kind::Null),
            (kind::BOOL, Kind::Bool),
            (kind::INT, Kind::Int),
            (kind::FLOAT, Kind::Float),
            (kind::STRING, Kind::String),
            (kind::TUPLE, Kind::Tuple),
            (kind::LIST, Kind::List),
            (kind::RECORD, Kind::Record),
            (kind::FUNCTION, Kind::Function),
            (kind::TAG, Kind::Tag),
            (kind::BYTES, Kind::Bytes),
            (kind::HANDLE, Kind::Handle),
        ];
        assert_eq!(kinds.len(), kind::NAMED.len());
        for (told, held) in kinds {
            assert_eq!(told, held as Word);
        }
    }
}
