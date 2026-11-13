//! The host's side of a native program: making and reading values, and
//! running native code for its externs.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use ambient_runtime::{Context, Fault, Word};
use serde::{Deserialize, Serialize};

use super::load::Program;
use crate::engine::{Native, Parts};
use crate::hash::Hash;
use crate::value::{self, Construct, Inspect, Value};

/// The program that called native code, as far as native code reaches
/// it. See "Native side" in `spec/externs.md`.
pub trait Running {
    /// Loads a function into the program and gives it as a value: one
    /// [`crate::engine::Engine::link_to_load`] made for the program's
    /// backend, with the native code for its externs. Its code stays
    /// while a value refers to it.
    ///
    /// # Errors
    ///
    /// Fails if the function cannot be loaded, or no program is
    /// running.
    fn load(&mut self, parts: &Parts, natives: &Natives) -> Result<Value, String>;
}

/// No program: what native code is handed when it is called again only
/// to see whether what it returned still holds.
pub struct Nowhere;

impl Running for Nowhere {
    fn load(&mut self, _: &Parts, _: &Natives) -> Result<Value, String> {
        Err("no program is running".to_owned())
    }
}

/// One call of native code: the program it was made from, and the
/// functions it loaded, each held until what it returns is made.
struct Call<'a> {
    program: &'a Program,
    held: Vec<Word>,
}

impl Running for Call<'_> {
    fn load(&mut self, parts: &Parts, natives: &Natives) -> Result<Value, String> {
        self.held.push(self.program.add(parts, natives)?);
        parts.function()
    }
}

/// Native code for one extern: the program that called it and its
/// arguments in, a result or what went wrong out.
pub type NativeFn = Rc<dyn Fn(&mut dyn Running, &[Value]) -> Result<Value, String>>;

/// Native code as a host offers it.
#[derive(Clone)]
pub struct Offered {
    pub run: NativeFn,
    /// Whether calling it again is cheap, has no side effects, and shows
    /// whether what it returned before still holds. See
    /// `spec/memoization.md`.
    pub verifiable: bool,
}

/// The native code a host offers, by the package that declares each
/// extern and the extern's tag.
#[derive(Clone, Default)]
pub struct Natives(HashMap<(String, String), Offered>);

impl Natives {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn offer(
        mut self,
        package: &str,
        tag: &str,
        verifiable: bool,
        native: impl Fn(&mut dyn Running, &[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        let run: NativeFn = Rc::new(native);
        let key = (package.to_owned(), tag.to_owned());
        self.0.insert(key, Offered { run, verifiable });
        self
    }

    /// Adds native code for the extern tagged `tag` in `package`. It is
    /// volatile: a run that called it is never reused.
    #[must_use]
    pub fn with(
        self,
        package: &str,
        tag: &str,
        native: impl Fn(&[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, tag, false, move |_, args| native(args))
    }

    /// Adds native code that is handed the program that called it, as
    /// well as its arguments. It is volatile, as [`Natives::with`] is.
    #[must_use]
    pub fn reaching(
        self,
        package: &str,
        tag: &str,
        native: impl Fn(&mut dyn Running, &[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, tag, false, native)
    }

    /// Adds native code that can be called again, cheaply and without
    /// side effects, to see whether what it returned still holds.
    #[must_use]
    pub fn verifiable(
        self,
        package: &str,
        tag: &str,
        native: impl Fn(&[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, tag, true, move |_, args| native(args))
    }

    /// The native code offered for one extern.
    #[must_use]
    pub fn get(&self, native: &Native) -> Option<&Offered> {
        self.0.get(&(native.package.clone(), native.tag.clone()))
    }

    /// The native code for each of `externs`, in their order.
    ///
    /// # Errors
    ///
    /// Fails if one is missing: the program cannot be linked.
    pub fn resolve(&self, externs: &[Native]) -> Result<Vec<Offered>, String> {
        externs
            .iter()
            .map(|native| {
                self.get(native).cloned().ok_or_else(|| {
                    format!(
                        "no native code for `{}` of package `{}`",
                        native.tag, native.package
                    )
                })
            })
            .collect()
    }
}

/// One call of native code: what a run observed of the world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub native: Native,
    pub args: Vec<Value>,
    pub result: Result<Value, String>,
    pub verifiable: bool,
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
    let args = match &mut value::decode(args, &mut Host) {
        Value::Tuple(args) => std::mem::take(args),
        _ => Vec::new(),
    };
    let index = usize::try_from(index).unwrap_or(usize::MAX);
    // Native code may load more into the program, so nothing of the
    // program is borrowed while it runs.
    let (native, offered) = program.natives.borrow().get(index).cloned().unzip();
    // Native code is not to unwind through compiled code.
    let mut call = Call {
        program,
        held: Vec::new(),
    };
    let outcome = match &offered {
        Some(offered) => catch_unwind(AssertUnwindSafe(|| (offered.run)(&mut call, &args)))
            .unwrap_or_else(|_| Err("native code panicked".to_owned())),
        None => Err("no such extern".to_owned()),
    };
    if let (Some(trace), Some(offered), Some(native)) =
        (program.trace.borrow_mut().as_mut(), &offered, native)
    {
        trace.push(Observation {
            native,
            args,
            result: outcome.clone(),
            verifiable: offered.verifiable,
        });
    }
    let result = match outcome {
        Ok(value) => value::encode(&value, &mut Inside(program)),
        Err(message) => {
            *program.failure.borrow_mut() = Some(message);
            // SAFETY: the context is the one this call was handed.
            unsafe { ambient_runtime::amb_fault(context, Fault::Native as u64) }
        }
    };
    // What was loaded is held from here on by whatever of it the result
    // refers to.
    for function in call.held {
        // SAFETY: each is a word `Program::add` gave this call.
        unsafe { ambient_runtime::amb_release(function) };
    }
    result
}

/// Makes words for one program, which is what it takes to make a
/// function: a function is a definition the program has.
pub struct Inside<'a>(pub &'a Program);

impl Construct for Inside<'_> {
    fn int(&mut self, value: i64) -> Word {
        Construct::int(&mut Host, value)
    }

    fn float(&mut self, bits: u64) -> Word {
        Construct::float(&mut Host, bits)
    }

    fn string(&mut self, bytes: &[u8]) -> Word {
        Construct::string(&mut Host, bytes)
    }

    fn key(&mut self, name: &[u8]) -> Word {
        Construct::key(&mut Host, name)
    }

    fn tag(&mut self, text: &[u8]) -> Word {
        Construct::tag(&mut Host, text)
    }

    fn sequence(&mut self, list: bool, length: u64) -> Word {
        Construct::sequence(&mut Host, list, length)
    }

    fn iterator(&mut self, source: u64) -> Word {
        Construct::iterator(&mut Host, source)
    }

    fn iterator_set(&mut self, iterator: Word, index: u64, value: Word) {
        Construct::iterator_set(&mut Host, iterator, index, value);
    }

    fn sequence_set(&mut self, sequence: Word, index: u64, value: Word) {
        Construct::sequence_set(&mut Host, sequence, index, value);
    }

    fn record(&mut self, length: u64) -> Word {
        Construct::record(&mut Host, length)
    }

    fn record_set(&mut self, record: Word, key: Word, value: Word) {
        Construct::record_set(&mut Host, record, key, value);
    }

    fn release(&mut self, word: Word) {
        Construct::release(&mut Host, word);
    }

    fn function(&mut self, definition: &Hash, captures: u64) -> Option<Word> {
        let definition = self.0.definition(definition)?;
        // SAFETY: the definition is one the program has, and every
        // capture is set before the closure is seen.
        Some(unsafe { ambient_runtime::amb_closure_new(definition, captures) })
    }

    fn capture(&mut self, function: Word, index: u64, value: Word) {
        // SAFETY: `encode` stays within the captures it asked for.
        unsafe { ambient_runtime::amb_closure_set(function, index, value) }
    }

    fn method(&mut self, function: Word) {
        // SAFETY: the function was just made, and nothing else holds it.
        unsafe { ambient_runtime::amb_method_mark(function) }
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

    fn definition(&mut self, word: Word) -> Word {
        // SAFETY: `decode` asks only about functions.
        unsafe { ambient_runtime::amb_closure_hash(word) }
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

    fn tag(&mut self, text: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_tag(text.as_ptr(), text.len() as u64) }
    }

    fn sequence(&mut self, list: bool, length: u64) -> Word {
        unsafe { ambient_runtime::amb_sequence_new(u64::from(list), length) }
    }

    fn iterator(&mut self, source: u64) -> Word {
        unsafe { ambient_runtime::amb_iterator_of(source) }
    }

    fn iterator_set(&mut self, iterator: Word, index: u64, value: Word) {
        unsafe { ambient_runtime::amb_iterator_hold(iterator, index, value) }
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
