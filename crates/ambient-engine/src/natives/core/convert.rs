//! Natives for `core::convert`: value rendering.
//!
//! The `TryFrom<String>` parsers (`parse_number`, `parse_bool`) moved to
//! their target primitives' native modules (`number.rs`, `bool.rs`), mirroring
//! the `.ab` move that keeps `core::convert` free of the `core::result` value
//! edge. Their uuids (0x0702, 0x0703) rode along unchanged.

use crate::{Value, VmError};

use super::{NativeRegistry, arg, bind, module};

// Uniform native signature; the wrap is the calling convention.
#[allow(clippy::unnecessary_wraps)]
fn to_string(mut args: Vec<Value>) -> Result<Value, VmError> {
    let value = arg(&mut args, 0);
    Ok(Value::string(crate::format::format_value(&value)))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "convert"]);
    bind(reg, &m, "to_string", 0x0701, 1, to_string);
}
