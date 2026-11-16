//! What `system.exchange` declares: a value as bytes and back, for a
//! program that keeps one or sends it. See "Values as bytes" in
//! `spec/targets.md`.
//!
//! Neither reaches a world, so each is offered as verifiable.

use super::code::invalid;
use super::{PACKAGE, bytes, error};
use crate::exchange;
use crate::native::Natives;
use crate::value::Value;

/// Whether a value is data all the way down: nothing in it is tied to
/// a program, as a function is to its definition and a cell to its
/// holder.
fn plain(value: &Value) -> bool {
    match value {
        Value::Null
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::Bytes(_)
        | Value::Tag { .. } => true,
        Value::Tuple(members) | Value::List(members) => members.iter().all(plain),
        Value::Record(fields) => fields.values().all(plain),
        _ => false,
    }
}

fn encode(value: Option<&Value>) -> Result<Vec<u8>, String> {
    let value = value.filter(|value| plain(value));
    let value = value.ok_or("only plain data has bytes: no function, cell, handle, or iterator")?;
    exchange::encode(value).map_err(|unsendable| format!("{unsendable:?}"))
}

fn decode(encoded: &[u8]) -> Result<Value, String> {
    let value = exchange::decode(encoded).map_err(|_| "these are not the bytes of a value")?;
    plain(&value)
        .then_some(value)
        .ok_or_else(|| "these are the bytes of what is not plain data".to_owned())
}

/// `natives`, with the functions of `system.exchange`.
pub(super) fn offer(natives: Natives) -> Natives {
    let answer = |done: Result<Value, String>| {
        done.unwrap_or_else(|message| error("exchange", &invalid(&message)))
    };
    natives
        .verifiable(PACKAGE, "exchange.encode", move |args| {
            Ok(answer(encode(args.first()).map(Value::Bytes)))
        })
        .verifiable(PACKAGE, "exchange.decode", move |args| {
            Ok(answer(decode(bytes(args, 0)?)))
        })
}
