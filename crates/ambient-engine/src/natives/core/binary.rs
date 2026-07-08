//! Natives for `core::primitives::binary`: the immutable byte buffer.

use crate::{Value, VmError};

use super::{
    NativeRegistry, binary_arg, bind, list, module, number, slice_bound, type_error, usize_index,
};

fn from(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "bytes_from")?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let bytes: Vec<u8> = l
        .iter()
        .map(|v| match v {
            Value::Number(n) => Ok(*n as u8),
            other => Err(type_error("Number", other, "bytes_from")),
        })
        .collect::<Result<Vec<u8>, VmError>>()?;
    Ok(Value::binary(bytes))
}

fn to_list(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "bytes_to_list")?;
    let list: Vec<Value> = bytes.iter().map(|&b| Value::Number(f64::from(b))).collect();
    Ok(Value::list(list))
}

fn length(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "bytes_length")?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Value::Number(bytes.len() as f64))
}

fn get(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "bytes_get")?;
    let index = number(&mut args, 1, "bytes_get")?;
    Ok(usize_index(index)
        .and_then(|index| bytes.get(index).copied())
        .map_or_else(Value::none, |b| Value::some(Value::Number(f64::from(b)))))
}

fn slice(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "bytes_slice")?;
    let start = number(&mut args, 1, "bytes_slice")?;
    let end = number(&mut args, 2, "bytes_slice")?;
    let len = bytes.len();
    let start = slice_bound(start, len);
    let end = slice_bound(end, len);
    Ok(Value::binary(if start < end {
        bytes[start..end].to_vec()
    } else {
        Vec::new()
    }))
}

fn concat(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = binary_arg(&mut args, 0, "bytes_concat")?;
    let b = binary_arg(&mut args, 1, "bytes_concat")?;
    let mut result = Vec::with_capacity(a.len() + b.len());
    result.extend_from_slice(&a);
    result.extend_from_slice(&b);
    Ok(Value::binary(result))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "primitives", "binary"]);
    bind(reg, &m, "from", 0x0601, 1, from);
    bind(reg, &m, "to_list", 0x0602, 1, to_list);
    bind(reg, &m, "length", 0x0603, 1, length);
    bind(reg, &m, "get", 0x0604, 2, get);
    bind(reg, &m, "slice", 0x0605, 3, slice);
    bind(reg, &m, "concat", 0x0606, 2, concat);
}
