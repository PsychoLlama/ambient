//! Values as the host sees them: what running a program gives back.

use std::collections::BTreeMap;

mod walk;

use ambient_runtime::{Kind, Word};

use crate::hash::Hash;
use serde::{Deserialize, Serialize};
use walk::{Step, build};

/// Cloning, comparing, and dropping one never recurses: see [`walk`].
///
/// TODO: formatting one for debugging and its derived serialization do
/// recurse, so a value nested deeper than the stack overflows them.
#[derive(Debug, Serialize, Deserialize)]
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
    Function(Function),
    /// A cell. It never leaves its program.
    Cell,
    /// What an abort handler is bound to: a hold on a provide that is
    /// still running. It never leaves its program.
    Token,
    /// The type arguments a generic function was made at, which follow
    /// what it captured. Nothing outside its program can read them yet.
    Types,
}

/// A function value: a definition, and what it captured where it was
/// ready. The bindings it captured are among the captures, as a record,
/// and then the type arguments of a generic one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    /// The hash of its definition, if it has one a host can name.
    pub definition: Option<Hash>,
    pub captures: Vec<Value>,
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
    /// The hash of a function value's definition, as a string.
    fn definition(&mut self, word: Word) -> Word;
}

/// The value a word represents.
pub fn decode(word: Word, inspect: &mut impl Inspect) -> Value {
    let text = |inspect: &mut _, word| {
        String::from_utf8_lossy(&Inspect::string(inspect, word)).into_owned()
    };
    build(word, |word| {
        let kind = inspect.kind(word);
        let words = |inspect: &mut _, from: u64, step: usize| -> Vec<Word> {
            (from..Inspect::length(inspect, word) * step as u64)
                .step_by(step)
                .map(|index| Inspect::word_at(inspect, word, index))
                .collect()
        };
        Step::Leaf(match kind {
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
            kind if kind == Kind::Tuple as u64 => {
                return Step::Inside(words(inspect, 0, 1), Box::new(Value::Tuple));
            }
            kind if kind == Kind::List as u64 => {
                return Step::Inside(words(inspect, 0, 1), Box::new(Value::List));
            }
            // A record's payload is each key and then its value.
            kind if kind == Kind::Record as u64 => {
                let keys: Vec<String> = words(inspect, 0, 2)
                    .into_iter()
                    .map(|key| text(inspect, key))
                    .collect();
                let record =
                    |values: Vec<Value>| Value::Record(keys.into_iter().zip(values).collect());
                return Step::Inside(words(inspect, 1, 2), Box::new(record));
            }
            // A function's payload is its definition, and then what it
            // captured.
            kind if kind == Kind::Function as u64 => {
                let hash = inspect.definition(word);
                let definition = Hash::parse(&text(inspect, hash));
                let function = move |captures| {
                    Value::Function(Function {
                        definition,
                        captures,
                    })
                };
                return Step::Inside(words(inspect, 1, 1), Box::new(function));
            }
            kind if kind == Kind::Cell as u64 => Value::Cell,
            kind if kind == Kind::Token as u64 => Value::Token,
            kind if kind == Kind::Types as u64 => Value::Types,
            _ => Value::Int(inspect.int(word)),
        })
    })
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
    /// A function over the definition with this hash, with room for
    /// `captures` values, or nothing if there is no such definition
    /// here. What it captures is trusted to be what its definition
    /// takes.
    fn function(&mut self, _definition: &Hash, _captures: u64) -> Option<Word> {
        None
    }
    fn capture(&mut self, _function: Word, _index: u64, _value: Word) {}
}

/// A container being filled: its word, and what is still to go in it.
struct Filling<'a> {
    word: Word,
    members: Members<'a>,
    /// Where the next member of a sequence goes.
    index: u64,
    /// The key the next field of a record goes under.
    key: Word,
}

enum Members<'a> {
    Sequence(std::slice::Iter<'a, Value>),
    Captures(std::slice::Iter<'a, Value>),
    Record(std::collections::btree_map::Iter<'a, String, Value>),
}

/// A value as a word the caller owns. A function is made only of a
/// definition `make` has; any other, and a cell or a token, which nothing
/// makes from outside a program, becomes null.
pub fn encode(value: &Value, make: &mut impl Construct) -> Word {
    let mut open: Vec<Filling> = Vec::new();
    let mut next = value;
    loop {
        let filling = |word, members| Filling {
            word,
            members,
            index: 0,
            key: ambient_runtime::NULL,
        };
        let mut ready = match next {
            Value::Null | Value::Cell | Value::Token | Value::Types => Some(ambient_runtime::NULL),
            Value::Function(function) => {
                let captures = function.captures.len() as u64;
                let definition = function.definition.as_ref();
                match definition.and_then(|hash| make.function(hash, captures)) {
                    Some(word) => {
                        open.push(filling(word, Members::Captures(function.captures.iter())));
                        None
                    }
                    None => Some(ambient_runtime::NULL),
                }
            }
            Value::Bool(true) => Some(ambient_runtime::TRUE),
            Value::Bool(false) => Some(ambient_runtime::FALSE),
            Value::Int(value) => Some(make.int(*value)),
            Value::Float(value) => Some(make.float(value.to_bits())),
            Value::String(text) => Some(make.string(text.as_bytes())),
            Value::Tag { package, tag } => {
                Some(make.tag(crate::literal::Literal::tag_text(package, tag).as_bytes()))
            }
            Value::Tuple(members) | Value::List(members) => {
                let list = matches!(next, Value::List(_));
                let word = make.sequence(list, members.len() as u64);
                open.push(filling(word, Members::Sequence(members.iter())));
                None
            }
            Value::Record(fields) => {
                let word = make.record(fields.len() as u64);
                open.push(filling(word, Members::Record(fields.iter())));
                None
            }
        };
        // Put what was ready where it goes, then find what to make next:
        // the next member of whatever is being filled, which is itself
        // ready once it has none.
        loop {
            let Some(parent) = open.last_mut() else {
                return ready.unwrap_or(ambient_runtime::NULL);
            };
            if let Some(word) = ready.take() {
                match parent.members {
                    Members::Sequence(_) => make.sequence_set(parent.word, parent.index, word),
                    Members::Captures(_) => make.capture(parent.word, parent.index, word),
                    Members::Record(_) => make.record_set(parent.word, parent.key, word),
                }
                parent.index += 1;
                make.release(word);
            }
            let member = match &mut parent.members {
                Members::Sequence(members) | Members::Captures(members) => members.next(),
                Members::Record(fields) => fields.next().map(|(name, member)| {
                    parent.key = make.key(name.as_bytes());
                    member
                }),
            };
            if let Some(member) = member {
                next = member;
                break;
            }
            ready = open.pop().map(|filled| filled.word);
        }
    }
}
