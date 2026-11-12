//! Values as the host sees them: what running a program gives back.

use ambient_runtime::{Kind, Word};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
}

/// Reads words. Each backend has its own way to reach the runtime.
pub trait Inspect {
    /// The [`Kind`] of a word, as its number.
    fn kind(&mut self, word: Word) -> u64;
    fn int(&mut self, word: Word) -> i64;
    fn float_bits(&mut self, word: Word) -> u64;
}

/// The value a word represents.
pub fn decode(word: Word, inspect: &mut impl Inspect) -> Value {
    let kind = inspect.kind(word);
    if kind == Kind::Null as u64 {
        Value::Null
    } else if kind == Kind::Bool as u64 {
        Value::Bool(word == ambient_runtime::TRUE)
    } else if kind == Kind::Float as u64 {
        Value::Float(f64::from_bits(inspect.float_bits(word)))
    } else {
        Value::Int(inspect.int(word))
    }
}
