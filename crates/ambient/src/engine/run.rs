//! Run: a function and its arguments yield a value. This is what a
//! comptime call is. See `spec/comptime.md` and `spec/memoization.md`.

use std::cell::OnceCell;

use super::{Engine, Stage};
use crate::backend::native::{Host, Native, Program, encode};
use crate::checker::Evaluate;
use crate::error::Error;
use crate::hash::{Hasher, encode as bytes};
use crate::symbol::Symbol;
use crate::value::{Value, decode};

/// The backend that runs code during a build, made on first use.
#[derive(Default)]
pub(super) struct Runner(OnceCell<Result<Native, String>>);

impl Engine<'_> {
    /// Calls `function` with `args` during the build.
    ///
    /// The result is kept under the function's hash and the arguments, so
    /// a call is made once however many builds ask for it.
    ///
    /// TODO: a run that performs abilities needs its trace kept with it;
    /// nothing can perform one yet.
    ///
    /// # Errors
    ///
    /// Fails if the function does not compile for this machine, or faults.
    pub fn run(&self, function: &Symbol, args: &[Value]) -> Result<Value, Error> {
        let mut key = Hasher::new("run");
        key.hash(&self.hash(function)?);
        key.bytes(&bytes(&args));
        let failed = |message: String| Error::Backend {
            symbol: function.clone(),
            message,
        };
        let native = match self.runner.0.get_or_init(Native::new) {
            Ok(native) => native,
            Err(message) => return Err(failed(message.clone())),
        };
        // Everything the program is made of is covered by the function's
        // hash, so linking here reads nothing the key does not cover.
        let parts = self.link(function, native)?;
        let result = self.stage(Stage::Run, function, key.finish(), || {
            let program = Program::load(&parts).map_err(failed)?;
            let words: Vec<u64> = args.iter().map(encode).collect();
            let result = program.call(&words);
            for word in words {
                // SAFETY: each word was made above and is not used again.
                unsafe { ambient_runtime::amb_release(word) };
            }
            let word = result.map_err(|fault| Error::ComptimeFault {
                function: function.clone(),
                fault: format!("{fault:?}"),
            })?;
            let value = decode(word, &mut Host);
            // SAFETY: the word is owned here and not used again.
            unsafe { ambient_runtime::amb_release(word) };
            Ok(value)
        })?;
        Ok((*result).clone())
    }
}

impl Evaluate for Engine<'_> {
    fn evaluate(&self, function: &Symbol, args: &[Value]) -> Result<Value, Error> {
        self.run(function, args)
    }
}
