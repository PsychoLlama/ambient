//! Exchange: the one encoding every unanchored value has. See
//! `spec/exchange.md`.
//!
//! The encoding is determined by a value's shape alone, so equal values
//! encode identically and the encoding gives every value a hash. Decoding
//! accepts exactly what encoding produces: bytes that decode at all are
//! the canonical bytes of the value they decode to.

use std::collections::BTreeMap;
use std::fmt;

use crate::hash::{Hash, Hasher};
use crate::value::Value;

/// Why a value has no encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsendable {
    /// It contains a cell, which is tied to the program that holds it.
    Cell,
    /// It contains a function. A host cannot read one back yet, so it
    /// has nothing to encode.
    Function,
}

impl fmt::Display for Unsendable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cell => write!(f, "a cell never leaves its program"),
            Self::Function => write!(f, "a function cannot be encoded yet"),
        }
    }
}

impl std::error::Error for Unsendable {}

/// Bytes that are not the encoding of any value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not the encoding of a value")
    }
}

impl std::error::Error for Malformed {}

const NULL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const INT: u8 = 3;
const FLOAT: u8 = 4;
const STRING: u8 = 5;
const TAG: u8 = 6;
const TUPLE: u8 = 7;
const LIST: u8 = 8;
const RECORD: u8 = 9;

fn text(text: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

/// The canonical encoding of `value`.
///
/// # Errors
///
/// Fails if the value is anchored, or holds a function.
pub fn encode(value: &Value) -> Result<Vec<u8>, Unsendable> {
    let mut out = Vec::new();
    // What is left to write, last first. A key is written before the
    // value it names.
    let mut pending = vec![(None, value)];
    while let Some((key, value)) = pending.pop() {
        if let Some(key) = key {
            text(key, &mut out);
        }
        let count = |count: usize, out: &mut Vec<u8>| {
            out.extend_from_slice(&(count as u64).to_le_bytes());
        };
        match value {
            Value::Null => out.push(NULL),
            Value::Bool(value) => out.push(if *value { TRUE } else { FALSE }),
            Value::Int(value) => {
                out.push(INT);
                out.extend_from_slice(&value.to_le_bytes());
            }
            // Equality is by bits, so the bits are the encoding.
            Value::Float(value) => {
                out.push(FLOAT);
                out.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            Value::String(value) => {
                out.push(STRING);
                text(value, &mut out);
            }
            Value::Tag { package, tag } => {
                out.push(TAG);
                text(package, &mut out);
                text(tag, &mut out);
            }
            Value::Tuple(members) | Value::List(members) => {
                out.push(if matches!(value, Value::List(_)) {
                    LIST
                } else {
                    TUPLE
                });
                count(members.len(), &mut out);
                pending.extend(members.iter().rev().map(|member| (None, member)));
            }
            // Fields go in the order of their names, whatever order they
            // were written in.
            Value::Record(fields) => {
                out.push(RECORD);
                count(fields.len(), &mut out);
                let fields = fields.iter().rev();
                pending.extend(fields.map(|(name, member)| (Some(name.as_str()), member)));
            }
            Value::Function => return Err(Unsendable::Function),
            Value::Cell => return Err(Unsendable::Cell),
        }
    }
    Ok(out)
}

/// The hash of a value: of its encoding.
///
/// # Errors
///
/// Fails where [`encode`] does.
pub fn hash(value: &Value) -> Result<Hash, Unsendable> {
    Ok(Hasher::new("value").bytes(&encode(value)?).finish())
}

/// A value whose members are still being read.
enum Partial {
    Sequence {
        list: bool,
        members: Vec<Value>,
    },
    Record {
        fields: BTreeMap<String, Value>,
        key: String,
    },
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], Malformed> {
        let (taken, rest) = self.0.split_at_checked(count).ok_or(Malformed)?;
        self.0 = rest;
        Ok(taken)
    }

    fn u64(&mut self) -> Result<u64, Malformed> {
        let bytes = self.take(8)?.try_into().map_err(|_| Malformed)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn text(&mut self) -> Result<String, Malformed> {
        let length = usize::try_from(self.u64()?).map_err(|_| Malformed)?;
        let bytes = self.take(length)?.to_vec();
        String::from_utf8(bytes).map_err(|_| Malformed)
    }
}

/// How many members are still to be read into the value on top.
struct Open {
    partial: Partial,
    remaining: u64,
}

impl Open {
    /// Reads the key of the next field, which must sort after the last:
    /// a record has one encoding.
    fn next_key(&mut self, reader: &mut Reader) -> Result<(), Malformed> {
        if let Partial::Record { fields, key } = &mut self.partial
            && self.remaining > 0
        {
            *key = reader.text()?;
            let ordered = fields.keys().next_back().is_none_or(|last| last < key);
            if !ordered {
                return Err(Malformed);
            }
        }
        Ok(())
    }

    fn finish(self) -> Value {
        match self.partial {
            Partial::Sequence {
                list: true,
                members,
            } => Value::List(members),
            Partial::Sequence { members, .. } => Value::Tuple(members),
            Partial::Record { fields, .. } => Value::Record(fields),
        }
    }
}

/// The value `bytes` is the canonical encoding of.
///
/// # Errors
///
/// Fails if they are not the canonical encoding of any value.
pub fn decode(bytes: &[u8]) -> Result<Value, Malformed> {
    let mut reader = Reader(bytes);
    let mut open: Vec<Open> = Vec::new();
    loop {
        let sequence = |list| Partial::Sequence {
            list,
            members: Vec::new(),
        };
        let mut value = match reader.take(1)?[0] {
            NULL => Value::Null,
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            INT => Value::Int(reader.u64()?.cast_signed()),
            FLOAT => Value::Float(f64::from_bits(reader.u64()?)),
            STRING => Value::String(reader.text()?),
            TAG => Value::Tag {
                package: reader.text()?,
                tag: reader.text()?,
            },
            kind @ (TUPLE | LIST | RECORD) => {
                let partial = match kind {
                    RECORD => Partial::Record {
                        fields: BTreeMap::new(),
                        key: String::new(),
                    },
                    _ => sequence(kind == LIST),
                };
                let mut started = Open {
                    partial,
                    remaining: reader.u64()?,
                };
                started.next_key(&mut reader)?;
                open.push(started);
                // An empty one is complete at once, like any value.
                match open.pop_if(|open| open.remaining == 0) {
                    Some(empty) => empty.finish(),
                    None => continue,
                }
            }
            _ => return Err(Malformed),
        };
        // The value is a member of whatever is open, and may complete it.
        loop {
            let Some(parent) = open.last_mut() else {
                return if reader.0.is_empty() {
                    Ok(value)
                } else {
                    Err(Malformed)
                };
            };
            match &mut parent.partial {
                Partial::Sequence { members, .. } => members.push(value),
                Partial::Record { fields, key } => {
                    fields.insert(std::mem::take(key), value);
                }
            }
            parent.remaining -= 1;
            parent.next_key(&mut reader)?;
            match open.pop_if(|open| open.remaining == 0) {
                Some(complete) => value = complete.finish(),
                None => break,
            }
        }
    }
}
