//! Calling native code. See `spec/externs.md`.
//!
//! Compiled code never holds the address of native code. It names an
//! extern by its position among the externs of the program, and the host
//! that loaded the program decides what runs.

use crate::{Context, Word};

/// What a host supplies to run native code: the context, the extern's
/// position, the caller's bindings, and the arguments as a tuple. It
/// returns a value the caller owns, or [`crate::UNWIND`] after reporting a
/// fault with [`crate::amb_fault`].
pub type Native = unsafe extern "C" fn(*mut Context, u64, Word, Word) -> Word;

/// In WebAssembly the host's function is an import of the runtime.
#[cfg(target_family = "wasm")]
mod host {
    use crate::{Context, Word};

    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        pub fn native(context: *mut Context, index: u64, bindings: Word, args: Word) -> Word;
    }
}

/// Natively the host's function is found through the context.
#[cfg(not(target_family = "wasm"))]
mod host {
    use crate::object::fault;
    use crate::{Context, Fault, Word};

    pub unsafe fn native(context: *mut Context, index: u64, bindings: Word, args: Word) -> Word {
        unsafe {
            let address = (*context).native as usize;
            if address == 0 {
                return fault(context, Fault::Native);
            }
            let run = core::mem::transmute::<usize, super::Native>(address);
            run(context, index, bindings, args)
        }
    }
}

/// Runs the extern at `index` with `args`, a tuple, and the caller's
/// `bindings`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_native(
    context: *mut Context,
    index: u64,
    bindings: Word,
    args: Word,
) -> Word {
    unsafe { host::native(context, index, bindings, args) }
}
