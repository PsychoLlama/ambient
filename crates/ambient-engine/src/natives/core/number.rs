//! Natives for `core::primitives::number`: the f64 math surface.

use crate::{Value, VmError};

use super::{NativeRegistry, bind, module, number};

fn unary(mut args: Vec<Value>, op: &'static str, f: fn(f64) -> f64) -> Result<Value, VmError> {
    let n = number(&mut args, 0, op)?;
    Ok(Value::Number(f(n)))
}

fn binary(
    mut args: Vec<Value>,
    op: &'static str,
    f: fn(f64, f64) -> f64,
) -> Result<Value, VmError> {
    let a = number(&mut args, 0, op)?;
    let b = number(&mut args, 1, op)?;
    Ok(Value::Number(f(a, b)))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "primitives", "number"]);
    bind(reg, &m, "sqrt", 0x0101, 1, |a| unary(a, "sqrt", f64::sqrt));
    bind(reg, &m, "abs", 0x0102, 1, |a| unary(a, "abs", f64::abs));
    bind(reg, &m, "floor", 0x0103, 1, |a| {
        unary(a, "floor", f64::floor)
    });
    bind(reg, &m, "ceil", 0x0104, 1, |a| unary(a, "ceil", f64::ceil));
    bind(reg, &m, "round", 0x0105, 1, |a| {
        unary(a, "round", f64::round)
    });
    bind(reg, &m, "trunc", 0x0106, 1, |a| {
        unary(a, "trunc", f64::trunc)
    });
    bind(reg, &m, "sin", 0x0107, 1, |a| unary(a, "sin", f64::sin));
    bind(reg, &m, "cos", 0x0108, 1, |a| unary(a, "cos", f64::cos));
    bind(reg, &m, "tan", 0x0109, 1, |a| unary(a, "tan", f64::tan));
    bind(reg, &m, "ln", 0x010A, 1, |a| unary(a, "ln", f64::ln));
    bind(reg, &m, "exp", 0x010B, 1, |a| unary(a, "exp", f64::exp));
    bind(reg, &m, "pow", 0x010C, 2, |a| binary(a, "pow", f64::powf));
    bind(reg, &m, "min", 0x010D, 2, |a| binary(a, "min", f64::min));
    bind(reg, &m, "max", 0x010E, 2, |a| binary(a, "max", f64::max));
    bind(reg, &m, "asin", 0x010F, 1, |a| unary(a, "asin", f64::asin));
    bind(reg, &m, "acos", 0x0110, 1, |a| unary(a, "acos", f64::acos));
    bind(reg, &m, "atan", 0x0111, 1, |a| unary(a, "atan", f64::atan));
    bind(reg, &m, "atan2", 0x0112, 2, |a| {
        binary(a, "atan2", f64::atan2)
    });
    bind(reg, &m, "log10", 0x0113, 1, |a| {
        unary(a, "log10", f64::log10)
    });
    bind(reg, &m, "log2", 0x0114, 1, |a| unary(a, "log2", f64::log2));
}
