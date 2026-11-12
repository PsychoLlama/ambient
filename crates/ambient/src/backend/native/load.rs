//! Loading compiled parts into this process, calling them, and reading
//! what they return.

use ambient_runtime::{Context, Entry, FALSE, Fault, NULL, TRUE, Word};
use memmap2::{Mmap, MmapMut};

use crate::backend::statics::{self, Field, Make};
use crate::engine::{Body, Parts};
use crate::value::{Inspect, Value};

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
}

impl Program {
    /// Loads the parts of a program compiled by [`Native`].
    ///
    /// # Errors
    ///
    /// Fails if executable memory cannot be mapped.
    pub fn load(parts: &Parts) -> Result<Self, String> {
        let io = |error: std::io::Error| error.to_string();
        let code_of = |index: usize| match &parts.parts[index].body {
            Body::Function { code, .. } => code.0.as_slice(),
            Body::Type(_) => &[],
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
            Body::Type(_) => return Err("the entry is not a function".to_owned()),
        };
        Ok(Self {
            entry_closure: (base + plan.entry * 8) as u64,
            entry_code: code_base + offsets[parts.entry],
            entry_arity: arity,
            entries: Entry::ALL.iter().map(|entry| entry.address()).collect(),
            _statics: block,
            _code: code,
        })
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
        assert_eq!(args.len(), self.entry_arity, "wrong number of arguments");
        let (code, closure) = (self.entry_code, self.entry_closure);
        // SAFETY: the code was compiled by `Native` for a function of this
        // arity, with the calling convention these types spell out, and the
        // context and closure are laid out as the runtime defines.
        unsafe {
            let context = ambient_runtime::amb_context_new();
            (*context).entries = self.entries.as_ptr() as u64;
            let result = match *args {
                [] => std::mem::transmute::<usize, extern "C" fn(C, u64) -> u64>(code)(
                    context, closure,
                ),
                [a] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64) -> u64>(code)(
                    context, closure, a,
                ),
                [a, b] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64, u64) -> u64>(
                    code,
                )(context, closure, a, b),
                [a, b, c] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, a, b, c),
                [a, b, c, d] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, a, b, c, d),
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

/// A value as a word the caller owns.
#[must_use]
pub fn encode(value: &Value) -> Word {
    let count = |length: usize| length as u64;
    // SAFETY: each constructor is given what it documents: lengths that
    // match the pointers, and indices within the lengths just allocated.
    unsafe {
        match value {
            Value::Null | Value::Function => NULL,
            Value::Bool(true) => TRUE,
            Value::Bool(false) => FALSE,
            Value::Int(value) => ambient_runtime::amb_int_new(*value),
            Value::Float(value) => ambient_runtime::amb_float_new(value.to_bits()),
            Value::String(text) => {
                ambient_runtime::amb_string_new(text.as_ptr(), count(text.len()))
            }
            Value::Tuple(members) | Value::List(members) => {
                let list = u64::from(matches!(value, Value::List(_)));
                let word = ambient_runtime::amb_sequence_new(list, count(members.len()));
                for (index, member) in members.iter().enumerate() {
                    let member = encode(member);
                    ambient_runtime::amb_sequence_set(word, count(index), member);
                    ambient_runtime::amb_release(member);
                }
                word
            }
            Value::Record(fields) => {
                let word = ambient_runtime::amb_record_new(count(fields.len()));
                for (name, member) in fields {
                    let key = ambient_runtime::amb_intern(name.as_ptr(), count(name.len()));
                    let member = encode(member);
                    ambient_runtime::amb_record_set(word, key, member);
                    ambient_runtime::amb_release(member);
                }
                word
            }
        }
    }
}

/// Reads words through the runtime linked into this process.
pub struct Host;

impl Inspect for Host {
    fn kind(&mut self, word: Word) -> u64 {
        // SAFETY: the word is a value a program returned.
        unsafe { ambient_runtime::amb_kind(word) }
    }

    fn int(&mut self, word: Word) -> i64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_int_value(word) }
    }

    fn float_bits(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_float_bits(word) }
    }

    fn length(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only about objects.
        unsafe { ambient_runtime::amb_length(word) }
    }

    fn word_at(&mut self, word: Word, index: u64) -> Word {
        // SAFETY: `decode` stays within the length it was told.
        unsafe { ambient_runtime::amb_word_at(word, index) }
    }

    fn string(&mut self, word: Word) -> Vec<u8> {
        // SAFETY: the word is a string, and its bytes are as long as its
        // length says.
        unsafe {
            let length = usize::try_from(ambient_runtime::amb_length(word)).unwrap_or(0);
            std::slice::from_raw_parts(ambient_runtime::amb_string_bytes(word), length).to_vec()
        }
    }
}
