//! Natives for `core::reflect`: runtime reflection over enum values.

use crate::{Value, VmError};

use super::{NativeRegistry, arg, bind, module, type_error};

fn tag(mut args: Vec<Value>) -> Result<Value, VmError> {
    match arg(&mut args, 0) {
        Value::Enum(e) => Ok(Value::Number(f64::from(e.tag))),
        other => Err(type_error("enum", &other, "enum_tag")),
    }
}

fn payload(mut args: Vec<Value>) -> Result<Value, VmError> {
    match arg(&mut args, 0) {
        Value::Enum(e) => {
            e.payload
                .as_deref()
                .cloned()
                .ok_or_else(|| VmError::EnumPayloadMissing {
                    type_name: e.type_name.to_string(),
                    variant_name: e.variant_name.to_string(),
                })
        }
        other => Err(type_error("enum", &other, "enum_payload")),
    }
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "reflect"]);
    bind(reg, &m, "tag", 0x0801, 1, tag);
    bind(reg, &m, "payload", 0x0802, 1, payload);
}
