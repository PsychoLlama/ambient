//! The host's side of a native program: making and reading values, and
//! running native code for its externs.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ambient_runtime::{Context, Fault, Word};

use super::load::Program;
use crate::engine::Native;
use crate::value::{self, Construct, Inspect, Value};

/// Native code for one extern: arguments in, a result or what went wrong
/// out.
pub type NativeFn = Box<dyn Fn(&[Value]) -> Result<Value, String>>;

/// The native code a host offers, by the package that declares each
/// extern and the extern's tag.
#[derive(Default)]
pub struct Natives(HashMap<(String, String), NativeFn>);

impl Natives {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds native code for the extern tagged `tag` in `package`.
    #[must_use]
    pub fn with(
        mut self,
        package: &str,
        tag: &str,
        native: impl Fn(&[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        let key = (package.to_owned(), tag.to_owned());
        self.0.insert(key, Box::new(native));
        self
    }

    /// The native code for each of `externs`, in their order.
    ///
    /// # Errors
    ///
    /// Fails if one is missing: the program cannot be linked.
    pub fn resolve(mut self, externs: &[Native]) -> Result<Vec<NativeFn>, String> {
        externs
            .iter()
            .map(|native| {
                let key = (native.package.clone(), native.tag.clone());
                self.0.remove(&key).ok_or_else(|| {
                    format!(
                        "no native code for `{}` of package `{}`",
                        native.tag, native.package
                    )
                })
            })
            .collect()
    }
}

/// What the runtime calls to run an extern: see
/// [`ambient_runtime::Native`].
pub(super) unsafe extern "C" fn dispatch(
    context: *mut Context,
    index: u64,
    _bindings: Word,
    args: Word,
) -> Word {
    // SAFETY: `Program::call` put itself in the context for the length of
    // the call this is part of.
    let program = unsafe {
        let address = usize::try_from((*context).host).unwrap_or(0);
        &*(address as *const Program)
    };
    let args = match value::decode(args, &mut Host) {
        Value::Tuple(args) => args,
        _ => Vec::new(),
    };
    let native = usize::try_from(index)
        .ok()
        .and_then(|index| program.natives.get(index));
    // Native code is not to unwind through compiled code.
    let outcome = match native {
        Some(native) => catch_unwind(AssertUnwindSafe(|| native(&args)))
            .unwrap_or_else(|_| Err("native code panicked".to_owned())),
        None => Err("no such extern".to_owned()),
    };
    match outcome {
        Ok(value) => encode(&value),
        Err(message) => {
            *program.failure.borrow_mut() = Some(message);
            // SAFETY: the context is the one this call was handed.
            unsafe { ambient_runtime::amb_fault(context, Fault::Native as u64) }
        }
    }
}

/// A value as a word the caller owns.
#[must_use]
pub fn encode(value: &Value) -> Word {
    value::encode(value, &mut Host)
}

/// Reads and makes words through the runtime linked into this process.
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

// SAFETY, throughout: each constructor is given what it documents: lengths
// that match the pointers, and indices within the lengths `encode` just
// asked for.
impl Construct for Host {
    fn int(&mut self, value: i64) -> Word {
        unsafe { ambient_runtime::amb_int_new(value) }
    }

    fn float(&mut self, bits: u64) -> Word {
        unsafe { ambient_runtime::amb_float_new(bits) }
    }

    fn string(&mut self, bytes: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_string_new(bytes.as_ptr(), bytes.len() as u64) }
    }

    fn key(&mut self, name: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_intern(name.as_ptr(), name.len() as u64) }
    }

    fn sequence(&mut self, list: bool, length: u64) -> Word {
        unsafe { ambient_runtime::amb_sequence_new(u64::from(list), length) }
    }

    fn sequence_set(&mut self, sequence: Word, index: u64, value: Word) {
        unsafe { ambient_runtime::amb_sequence_set(sequence, index, value) }
    }

    fn record(&mut self, length: u64) -> Word {
        unsafe { ambient_runtime::amb_record_new(length) }
    }

    fn record_set(&mut self, record: Word, key: Word, value: Word) {
        unsafe { ambient_runtime::amb_record_set(record, key, value) }
    }

    fn release(&mut self, word: Word) {
        unsafe { ambient_runtime::amb_release(word) }
    }
}
