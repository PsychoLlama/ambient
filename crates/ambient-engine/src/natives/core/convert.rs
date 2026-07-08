//! Natives for `core::convert`: primitive value conversions.

use crate::{Value, VmError};

use super::{NativeRegistry, arg, bind, module, string};

// Uniform native signature; the wrap is the calling convention.
#[allow(clippy::unnecessary_wraps)]
fn to_string(mut args: Vec<Value>) -> Result<Value, VmError> {
    let value = arg(&mut args, 0);
    Ok(Value::string(crate::format::format_value(&value)))
}

fn parse_number(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "parse_number")?;
    Ok(match s.trim().parse::<f64>() {
        Ok(n) => Value::some(Value::Number(n)),
        Err(_) => Value::none(),
    })
}

fn parse_bool(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "parse_bool")?;
    Ok(match s.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" => Value::some(Value::Bool(true)),
        "false" | "0" | "no" => Value::some(Value::Bool(false)),
        _ => Value::none(),
    })
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "convert"]);
    bind(reg, &m, "to_string", 0x0701, 1, to_string);
    bind(reg, &m, "parse_number", 0x0702, 1, parse_number);
    bind(reg, &m, "parse_bool", 0x0703, 1, parse_bool);
}
