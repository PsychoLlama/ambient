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
    /// A tag, and the package it belongs to.
    Tag {
        package: String,
        tag: String,
    },
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
    /// A function. What it does cannot be read back.
    Function,
    /// A cell. It never leaves its program.
    Cell,
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
    /// The bytes of a string, or of a tag.
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
        kind if kind == Kind::Tag as u64 => {
            let text = text(inspect, word);
            let (package, tag) = text.split_once('\0').unwrap_or(("", &text));
            Value::Tag {
                package: package.to_owned(),
                tag: tag.to_owned(),
            }
        }
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
        kind if kind == Kind::Cell as u64 => Value::Cell,
        _ => Value::Int(inspect.int(word)),
    }
}

/// Makes words. Each backend has its own way to reach the runtime.
pub trait Construct {
    fn int(&mut self, value: i64) -> Word;
    fn float(&mut self, bits: u64) -> Word;
    fn string(&mut self, bytes: &[u8]) -> Word;
    /// The key a record holds the field `name` under.
    fn key(&mut self, name: &[u8]) -> Word;
    /// The tag with this text: see [`crate::literal::Literal::tag_text`].
    fn tag(&mut self, text: &[u8]) -> Word;
    /// A tuple, or a list, of `length` nulls.
    fn sequence(&mut self, list: bool, length: u64) -> Word;
    fn sequence_set(&mut self, sequence: Word, index: u64, value: Word);
    /// A record with room for `length` fields.
    fn record(&mut self, length: u64) -> Word;
    fn record_set(&mut self, record: Word, key: Word, value: Word);
    fn release(&mut self, word: Word);
}

/// A value as a word the caller owns. A function has no word to be made
/// from, and becomes null.
pub fn encode(value: &Value, make: &mut impl Construct) -> Word {
    match value {
        Value::Null | Value::Function | Value::Cell => ambient_runtime::NULL,
        Value::Bool(true) => ambient_runtime::TRUE,
        Value::Bool(false) => ambient_runtime::FALSE,
        Value::Int(value) => make.int(*value),
        Value::Float(value) => make.float(value.to_bits()),
        Value::String(text) => make.string(text.as_bytes()),
        Value::Tag { package, tag } => {
            make.tag(crate::literal::Literal::tag_text(package, tag).as_bytes())
        }
        Value::Tuple(members) | Value::List(members) => {
            let list = matches!(value, Value::List(_));
            let word = make.sequence(list, members.len() as u64);
            for (index, member) in members.iter().enumerate() {
                let member = encode(member, make);
                make.sequence_set(word, index as u64, member);
                make.release(member);
            }
            word
        }
        Value::Record(fields) => {
            let word = make.record(fields.len() as u64);
            for (name, member) in fields {
                let key = make.key(name.as_bytes());
                let member = encode(member, make);
                make.record_set(word, key, member);
                make.release(member);
            }
            word
        }
    }
}
