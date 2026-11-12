//! Values as the host sees them: what running a program gives back.

use std::collections::BTreeMap;

use ambient_runtime::{Kind, Word};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
    /// A function. What it does cannot be read back.
    Function,
}

/// Reads words. Each backend has its own way to reach the runtime.
pub trait Inspect {
    /// The [`Kind`] of a word, as its number.
    fn kind(&mut self, word: Word) -> u64;
    fn int(&mut self, word: Word) -> i64;
    fn float_bits(&mut self, word: Word) -> u64;
    /// How many members, fields, or bytes an object has.
    fn length(&mut self, word: Word) -> u64;
    /// A word of an object's payload.
    fn word_at(&mut self, word: Word, index: u64) -> Word;
    /// The bytes of a string.
    fn string(&mut self, word: Word) -> Vec<u8>;
}

/// The value a word represents.
pub fn decode(word: Word, inspect: &mut impl Inspect) -> Value {
    let kind = inspect.kind(word);
    let text = |inspect: &mut _, word| {
        String::from_utf8_lossy(&Inspect::string(inspect, word)).into_owned()
    };
    let members = |inspect: &mut _, word| {
        (0..Inspect::length(inspect, word))
            .map(|index| {
                let member = Inspect::word_at(inspect, word, index);
                decode(member, inspect)
            })
            .collect()
    };
    match kind {
        kind if kind == Kind::Null as u64 => Value::Null,
        kind if kind == Kind::Bool as u64 => Value::Bool(word == ambient_runtime::TRUE),
        kind if kind == Kind::Float as u64 => {
            Value::Float(f64::from_bits(inspect.float_bits(word)))
        }
        kind if kind == Kind::String as u64 => Value::String(text(inspect, word)),
        kind if kind == Kind::Tuple as u64 => Value::Tuple(members(inspect, word)),
        kind if kind == Kind::List as u64 => Value::List(members(inspect, word)),
        kind if kind == Kind::Record as u64 => {
            let mut fields = BTreeMap::new();
            for index in 0..inspect.length(word) {
                let key = inspect.word_at(word, index * 2);
                let value = inspect.word_at(word, index * 2 + 1);
                fields.insert(text(inspect, key), decode(value, inspect));
            }
            Value::Record(fields)
        }
        kind if kind == Kind::Function as u64 => Value::Function,
        _ => Value::Int(inspect.int(word)),
    }
}
