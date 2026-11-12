//! Loading compiled parts into this process, calling them, and reading
//! what they return.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet};

use ambient_runtime::{Context, Entry, Fault, NULL, Word};

use memmap2::{Mmap, MmapMut};

use super::host::{Host, Natives, Observation, Offered, dispatch};
use crate::backend::statics::{self, Base, Field, Known, Make, Place};
use crate::engine::{Body, Native, Part, Parts};
use crate::hash::Hash;
use crate::value::{self, Value};

/// What one load brought: code, and the block of words it reads.
struct Unit {
    /// Held so that the code stays mapped.
    _code: Mmap,
    /// Every definition and its closure. Held so that they stay put.
    _statics: Box<[u64]>,
}

/// A program loaded into this process, ready to call. More can be
/// loaded into it while it lives: see [`Program::add`].
pub struct Program {
    units: RefCell<Vec<Unit>>,
    /// Where each definition it has is, by hash.
    known: RefCell<HashMap<Hash, Known>>,
    /// The functions among them that have a metered form.
    metered: RefCell<HashSet<Hash>>,
    entries: Box<[usize]>,
    /// The closure of the function it was first loaded for.
    entry: Cell<Word>,
    /// The native code for each extern, by its position in the program.
    pub(super) natives: RefCell<Vec<Offered>>,
    /// What each of those externs is, in the same order.
    pub(super) externs: RefCell<Vec<Native>>,
    /// Every call of native code so far, if they are being kept.
    pub(super) trace: RefCell<Option<Vec<Observation>>>,
    /// What native code last said went wrong.
    pub(super) failure: RefCell<Option<String>>,
    /// The fault that ended the last call, as an error.
    error: RefCell<Option<Value>>,
    /// The first of the cells calls have made that are still live, which
    /// the runtime keeps up to date: a cell outlives the call that made
    /// it, and none outlives the program.
    cells: Box<UnsafeCell<u64>>,
}

impl Drop for Program {
    fn drop(&mut self) {
        // SAFETY: the word is the one every call's context was given.
        // No call is running, so nothing reads a cell after this.
        unsafe { ambient_runtime::amb_cells_drop(self.cells.get()) }
    }
}

impl Program {
    /// Loads the parts of a program compiled by [`super::Native`], with
    /// the native code for its externs.
    ///
    /// # Errors
    ///
    /// Fails if `natives` lacks an extern the program reaches, or
    /// executable memory cannot be mapped.
    pub fn load(parts: &Parts, natives: &Natives) -> Result<Self, String> {
        let program = Self {
            units: RefCell::default(),
            known: RefCell::default(),
            metered: RefCell::default(),
            entries: Entry::ALL.iter().map(|entry| entry.address()).collect(),
            entry: Cell::new(NULL),
            natives: RefCell::default(),
            externs: RefCell::default(),
            trace: RefCell::default(),
            failure: RefCell::default(),
            error: RefCell::default(),
            cells: Box::default(),
        };
        program.entry.set(program.add(parts, natives)?);
        Ok(program)
    }

    /// Loads more into the program: the parts of a function and what it
    /// reaches, as [`crate::engine::Engine::link_to_load`] makes them.
    /// Returns the function, a value that lives as long as the program.
    ///
    /// A part the program has already, by hash, is not loaded again;
    /// what is loaded uses the one that is there. What is loaded stays
    /// until the program is dropped.
    ///
    /// # Errors
    ///
    /// Fails if `natives` lacks an extern the parts reach, or executable
    /// memory cannot be mapped. Nothing is loaded then.
    pub fn add(&self, parts: &Parts, natives: &Natives) -> Result<Word, String> {
        let offered = natives.resolve(&parts.natives)?;
        let io = |error: std::io::Error| error.to_string();
        let wants_metered =
            |part: &Part| matches!(&part.body, Body::Function { metered, .. } if metered.is_some());
        // A function that is here without the metered form these parts
        // call it by is loaded again, with one.
        let lacking: HashSet<Hash> = parts
            .parts
            .iter()
            .filter(|part| wants_metered(part) && !self.metered.borrow().contains(&part.hash))
            .map(|part| part.hash)
            .collect();
        let known = |hash: &Hash| {
            let known = self.known.borrow().get(hash).copied();
            known.filter(|_| !lacking.contains(hash))
        };
        let plan = statics::plan(
            parts,
            &Base {
                known: &known,
                natives: self.natives.borrow().len(),
            },
        );

        let codes = parts.codes();
        let code_of = |index: usize| match plan.placed[index / 2] {
            Some(_) => codes[index].map_or(&[][..], |(_, code)| &code.0),
            None => &[][..],
        };
        let mut offsets = Vec::new();
        let mut size = 0usize;
        for index in 0..codes.len() {
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

        let mut block = vec![0u64; plan.words.max(1)].into_boxed_slice();
        let base = block.as_ptr() as usize;
        let address = |word: usize| (base + word * 8) as u64;
        for (word, field) in &plan.fields {
            block[*word] = match *field {
                Field::Word(value) => value,
                Field::Address(target) => address(target),
                Field::Code(code) => (code_base + offsets[code]) as u64,
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
                            Make::Tag => ambient_runtime::amb_tag(text.as_ptr(), length as u64),
                        }
                    }
                }
            };
        }

        for (part, placed) in parts.parts.iter().zip(&plan.placed) {
            let Some((root, definition)) = *placed else {
                continue;
            };
            let known = Known {
                root: address(root),
                definition: address(definition),
            };
            self.known.borrow_mut().insert(part.hash, known);
            if wants_metered(part) {
                self.metered.borrow_mut().insert(part.hash);
            }
        }
        self.natives.borrow_mut().extend(offered);
        self.externs
            .borrow_mut()
            .extend(parts.natives.iter().cloned());
        self.units.borrow_mut().push(Unit {
            _code: code,
            _statics: block,
        });
        match plan.entry {
            Some(Place::Here(word)) => Ok(address(word)),
            Some(Place::There(closure)) => Ok(closure),
            None => Err("the parts have no entry".to_owned()),
        }
    }

    /// A function the program has, by the hash of its definition: a
    /// value that lives as long as the program.
    #[must_use]
    pub fn function(&self, hash: &Hash) -> Option<Word> {
        self.known.borrow().get(hash).map(|known| known.root)
    }

    /// Where the definition of a function the program has is.
    pub(super) fn definition(&self, hash: &Hash) -> Option<u64> {
        let known = self.known.borrow().get(hash).copied();
        known.map(|known| known.definition).filter(|at| *at != 0)
    }

    /// What native code said went wrong, if the last call ended with
    /// [`Fault::Native`].
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    /// The fault that ended the last call, as the error a boundary
    /// would have made of it: its kind, and the trace of where it was.
    #[must_use]
    pub fn error(&self) -> Option<Value> {
        self.error.borrow().clone()
    }

    /// Keeps every call of native code from now on, for
    /// [`Self::observations`].
    pub fn observe(&self) {
        *self.trace.borrow_mut() = Some(Vec::new());
    }

    /// Every call of native code since [`Self::observe`], in order.
    #[must_use]
    pub fn observations(&self) -> Vec<Observation> {
        self.trace.borrow().clone().unwrap_or_default()
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
        self.call_value(self.entry.get(), args)
    }

    /// Calls a function value of this program with `args`, which it
    /// borrows, and with nothing bound: what [`Self::call`] does with
    /// the entry.
    ///
    /// # Errors
    ///
    /// Returns the fault if the function faulted.
    ///
    /// # Panics
    ///
    /// Panics if `function` is not a function, or `args` is not as many
    /// words as it takes, or more than four.
    pub fn call_value(&self, function: Word, args: &[Word]) -> Result<Word, Fault> {
        type C = *mut Context;
        // SAFETY: a value's kind can always be asked.
        let kind = unsafe { ambient_runtime::amb_kind(function) };
        assert_eq!(
            kind,
            ambient_runtime::Kind::Function as u64,
            "not a function"
        );
        let mut arity = 0;
        // SAFETY: it is a function, and `arity` is a word to write to.
        let code = unsafe { ambient_runtime::amb_closure_code(function, &raw mut arity) };
        let word = |word: u64| usize::try_from(word).unwrap_or(0);
        let (code, arity) = (word(code), word(arity));
        // The function is handed no bindings: nothing is provided to it.
        let mut words = vec![NULL];
        words.extend_from_slice(args);
        assert_eq!(words.len(), arity, "wrong number of arguments");
        let closure = function;
        // SAFETY: the code was compiled by `Native` for a function of this
        // arity, with the calling convention these types spell out, and the
        // context and closure are laid out as the runtime defines.
        unsafe {
            let context = ambient_runtime::amb_context_new();
            (*context).entries = self.entries.as_ptr() as u64;
            (*context).native = dispatch as *const () as u64;
            (*context).host = std::ptr::from_ref(self) as u64;
            (*context).cells = self.cells.get() as u64;
            let invoke = move || match *words {
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
            // Unwinding in this mode is the root task suspending: the
            // others take their turns, and the entry runs again to
            // resume it where it was.
            let mut result = invoke();
            while result == ambient_runtime::UNWIND
                && (*context).mode == ambient_runtime::SUSPENDING
            {
                (*context).mode = ambient_runtime::RUNNING;
                ambient_runtime::amb_schedule(context);
                result = invoke();
            }
            let fault = Fault::from_code((*context).fault);
            // The root is a boundary: a fault that reached it is an
            // error, for whoever called.
            let error = (result == ambient_runtime::UNWIND)
                .then(|| ambient_runtime::amb_context_error(context))
                .filter(|error| *error != NULL)
                .map(|word| {
                    let error = value::decode(word, &mut Host);
                    ambient_runtime::amb_release(word);
                    error
                });
            *self.error.borrow_mut() = error;
            ambient_runtime::amb_context_free(context);
            if result == ambient_runtime::UNWIND {
                Err(fault)
            } else {
                Ok(result)
            }
        }
    }
}
