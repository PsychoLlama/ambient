//! Natives for `core::protocol`: wire serialization and closure/handler
//! introspection.

use crate::{Value, VmError};

use super::{NativeRegistry, arg, binary_arg, bind, module, string, type_error};

fn serialize_value(mut args: Vec<Value>) -> Result<Value, VmError> {
    let value = arg(&mut args, 0);
    let bytes =
        crate::protocol::serialize_value(&value).map_err(|kind| VmError::TypeErrorOwned {
            expected: "wire-serializable value".to_string(),
            got: format!("{kind} (cannot cross the wire)"),
        })?;
    Ok(Value::binary(bytes))
}

fn deserialize_value(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "deserialize_value")?;
    Ok(match crate::protocol::deserialize_value(&bytes) {
        Some(value) => Value::some(value),
        None => Value::none(),
    })
}

fn closure_hash(mut args: Vec<Value>) -> Result<Value, VmError> {
    match arg(&mut args, 0) {
        Value::Closure(c) => Ok(Value::string(c.function_hash.to_string())),
        // A bare function reference is its own hash — the zero-capture
        // spelling of the same question.
        Value::FunctionRef(hash) => Ok(Value::string(hash.to_string())),
        other => Err(type_error("closure", &other, "closure_hash")),
    }
}

fn closure_captures(mut args: Vec<Value>) -> Result<Value, VmError> {
    let environment = match arg(&mut args, 0) {
        Value::Closure(c) => c.environment.clone(),
        Value::FunctionRef(_) => Vec::new(),
        other => return Err(type_error("closure", &other, "closure_captures")),
    };
    let captures_value = Value::list(environment);
    let bytes = crate::protocol::serialize_value(&captures_value).map_err(|kind| {
        VmError::TypeErrorOwned {
            expected: "wire-serializable captures".to_string(),
            got: format!("{kind} (cannot cross the wire)"),
        }
    })?;
    Ok(Value::binary(bytes))
}

fn handler_methods(mut args: Vec<Value>) -> Result<Value, VmError> {
    let handler = match arg(&mut args, 0) {
        Value::Handler(h) => h,
        other => return Err(type_error("handler", &other, "handler_methods")),
    };
    let mut methods: Vec<_> = handler.methods.iter().collect();
    methods.sort_by_key(|(id, _)| **id);
    let hashes: Vec<Value> = methods
        .into_iter()
        .map(|(_, hash)| Value::string(hash.to_hex().to_string()))
        .collect();
    Ok(Value::list(hashes))
}

fn hex_to_binary(mut args: Vec<Value>) -> Result<Value, VmError> {
    let hex_str = string(&mut args, 0, "hex_to_binary")?;
    Ok(match hex::decode(&*hex_str) {
        Ok(bytes) => Value::some(Value::binary(bytes)),
        Err(_) => Value::none(),
    })
}

fn binary_to_hex(mut args: Vec<Value>) -> Result<Value, VmError> {
    let bytes = binary_arg(&mut args, 0, "binary_to_hex")?;
    Ok(Value::string(hex::encode(bytes.as_ref())))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "protocol"]);
    bind(reg, &m, "serialize_value", 0x0901, 1, serialize_value);
    bind(reg, &m, "deserialize_value", 0x0902, 1, deserialize_value);
    bind(reg, &m, "closure_hash", 0x0903, 1, closure_hash);
    bind(reg, &m, "closure_captures", 0x0904, 1, closure_captures);
    bind(reg, &m, "handler_methods", 0x0905, 1, handler_methods);
    bind(reg, &m, "hex_to_binary", 0x0906, 1, hex_to_binary);
    bind(reg, &m, "binary_to_hex", 0x0907, 1, binary_to_hex);
}
