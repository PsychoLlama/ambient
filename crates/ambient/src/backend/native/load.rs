//! Loading compiled parts into this process, calling them, and reading
//! what they return.

use ambient_runtime::{Context, Entry, Fault, NULL, Word};
use std::cell::RefCell;

use memmap2::{Mmap, MmapMut};

use super::host::{NativeFn, Natives, dispatch};
use crate::backend::statics::{self, Field, Make};
use crate::engine::{Body, Parts};

/// A program loaded into this process, ready to call.
pub struct Program {
    /// The code. Held so that it stays mapped.
    _code: Mmap,
    /// Every definition and its closure. Held so that they stay put.
    _statics: Box<[u64]>,
    entries: Box<[usize]>,
    entry_closure: u64,
    entry_code: usize,
    entry_arity: usize,
    /// The native code for each extern, by its position in the program.
    pub(super) natives: Vec<NativeFn>,
    /// What native code last said went wrong.
    pub(super) failure: RefCell<Option<String>>,
}

impl Program {
    /// Loads the parts of a program compiled by [`super::Native`], with
    /// the native code for its externs.
    ///
    /// # Errors
    ///
    /// Fails if `natives` lacks an extern the program reaches, or
    /// executable memory cannot be mapped.
    pub fn load(parts: &Parts, natives: Natives) -> Result<Self, String> {
        let natives = natives.resolve(&parts.natives)?;
        let io = |error: std::io::Error| error.to_string();
        let code_of = |index: usize| match &parts.parts[index].body {
            Body::Function { code, .. } => code.0.as_slice(),
            Body::Type(_) | Body::Inert => &[],
        };
        let mut offsets = Vec::new();
        let mut size = 0usize;
        for index in 0..parts.parts.len() {
            offsets.push(size);
            size += code_of(index).len().next_multiple_of(16);
        }
        let mut code = MmapMut::map_anon(size.max(1)).map_err(io)?;
        for (index, offset) in offsets.iter().enumerate() {
            let bytes = code_of(index);
            code[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        let code = code.make_exec().map_err(io)?;
        let code_base = code.as_ptr() as usize;

        let plan = statics::plan(parts);
        let mut block = vec![0u64; plan.words.max(1)].into_boxed_slice();
        let base = block.as_ptr() as usize;
        for (word, field) in &plan.fields {
            block[*word] = match *field {
                Field::Word(value) => value,
                Field::Address(target) => (base + target * 8) as u64,
                Field::Code(part) => (code_base + offsets[part]) as u64,
                Field::Text {
                    make,
                    offset,
                    length,
                } => {
                    let text = &plan.text[offset..offset + length];
                    // SAFETY: the pointer and length are those of a live
                    // slice, which the runtime copies.
                    unsafe {
                        match make {
                            Make::String => {
                                ambient_runtime::amb_string_pinned(text.as_ptr(), length as u64)
                            }
                            Make::Field => {
                                ambient_runtime::amb_intern(text.as_ptr(), length as u64)
                            }
                        }
                    }
                }
            };
        }

        let arity = match &parts.parts[parts.entry].body {
            Body::Function { arity, .. } => *arity,
            Body::Type(_) | Body::Inert => return Err("the entry is not a function".to_owned()),
        };
        Ok(Self {
            entry_closure: (base + plan.entry * 8) as u64,
            entry_code: code_base + offsets[parts.entry],
            entry_arity: arity,
            entries: Entry::ALL.iter().map(|entry| entry.address()).collect(),
            natives,
            failure: RefCell::default(),
            _statics: block,
            _code: code,
        })
    }

    /// What native code said went wrong, if the last call ended with
    /// [`Fault::Native`].
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    /// Calls the entry with `args`, which it borrows. The result is a word
    /// the caller owns, or the fault that ended the call.
    ///
    /// # Errors
    ///
    /// Returns the fault if the program faulted.
    ///
    /// # Panics
    ///
    /// Panics if `args` is not as many words as the entry takes, or more
    /// than four.
    pub fn call(&self, args: &[Word]) -> Result<Word, Fault> {
        type C = *mut Context;
        // The entry is handed no bindings: nothing is provided to it.
        let mut words = vec![NULL];
        words.extend_from_slice(args);
        assert_eq!(words.len(), self.entry_arity, "wrong number of arguments");
        let (code, closure) = (self.entry_code, self.entry_closure);
        // SAFETY: the code was compiled by `Native` for a function of this
        // arity, with the calling convention these types spell out, and the
        // context and closure are laid out as the runtime defines.
        unsafe {
            let context = ambient_runtime::amb_context_new();
            (*context).entries = self.entries.as_ptr() as u64;
            (*context).native = dispatch as *const () as u64;
            (*context).host = std::ptr::from_ref(self) as u64;
            let result = match *words {
                [w0] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64) -> u64>(code)(
                    context, closure, w0,
                ),
                [w0, w1] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64, u64) -> u64>(
                    code,
                )(context, closure, w0, w1),
                [w0, w1, w2] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, w0, w1, w2),
                [w0, w1, w2, w3] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, w0, w1, w2, w3),
                [w0, w1, w2, w3, w4] => {
                    std::mem::transmute::<
                        usize,
                        extern "C" fn(C, u64, u64, u64, u64, u64, u64) -> u64,
                    >(code)(context, closure, w0, w1, w2, w3, w4)
                }
                _ => panic!("the host can pass at most four arguments"),
            };
            let fault = Fault::from_code((*context).fault);
            ambient_runtime::amb_context_free(context);
            if result == ambient_runtime::UNWIND {
                Err(fault)
            } else {
                Ok(result)
            }
        }
    }
}
