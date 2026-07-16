//! Natives for `core::primitives::bool`: the `Bool` parser.
//!
//! `Bool` itself is a source-constructorless primitive, so its only native is
//! `parse_bool` — bound here, with its target type, after moving off
//! `core::convert` so that module keeps clear of the `core::result` value edge
//! (see `convert.rs`). Uuid 0x0703 is unchanged from that move. The `.ab`
//! `TryFrom<String> for Bool` impl wraps the `Option` this returns.

use crate::{Value, VmError};

use super::{NativeRegistry, bind, module, string};

fn parse_bool(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "parse_bool")?;
    Ok(match s.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" => Value::some(Value::Bool(true)),
        "false" | "0" | "no" => Value::some(Value::Bool(false)),
        _ => Value::none(),
    })
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "primitives", "bool"]);
    bind(reg, &m, "parse_bool", 0x0703, 1, parse_bool);
}
