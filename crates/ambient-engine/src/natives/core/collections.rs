//! Natives for `core::collections::{list, map, set}`.

use std::sync::Arc;

use crate::{Value, VmError};

use super::{
    NativeRegistry, arg, bind, list, module, number, slice_bound, type_error, usize_index,
};

// ─────────────────────────────────────────────────────────────────────────
// list
// ─────────────────────────────────────────────────────────────────────────

fn list_length(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_length")?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Value::Number(l.len() as f64))
}

fn list_get(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_get")?;
    let index = number(&mut args, 1, "list_get")?;
    Ok(usize_index(index)
        .and_then(|index| l.get(index).cloned())
        .map_or_else(Value::none, Value::some))
}

fn list_head(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_head")?;
    Ok(l.first().cloned().map_or_else(Value::none, Value::some))
}

fn list_tail(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_tail")?;
    Ok(Value::list(if l.len() <= 1 {
        Vec::new()
    } else {
        l[1..].to_vec()
    }))
}

fn list_last(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_last")?;
    Ok(l.last().cloned().map_or_else(Value::none, Value::some))
}

fn list_concat(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = list(&mut args, 0, "list_concat")?;
    let b = list(&mut args, 1, "list_concat")?;
    let mut result = (*a).clone();
    result.extend(b.iter().cloned());
    Ok(Value::list(result))
}

fn list_append(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_append")?;
    let value = arg(&mut args, 1);
    let mut result = (*l).clone();
    result.push(value);
    Ok(Value::list(result))
}

fn list_is_empty(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_is_empty")?;
    Ok(Value::Bool(l.is_empty()))
}

fn list_reverse(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_reverse")?;
    let mut result = (*l).clone();
    result.reverse();
    Ok(Value::list(result))
}

fn list_sort(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_sort")?;
    let mut result = (*l).clone();
    result.sort_by(|a, b| match (a, b) {
        (Value::Number(na), Value::Number(nb)) => {
            na.partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Value::String(sa), Value::String(sb)) => sa.cmp(sb),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(Value::list(result))
}

fn list_slice(mut args: Vec<Value>) -> Result<Value, VmError> {
    let l = list(&mut args, 0, "list_slice")?;
    let start = number(&mut args, 1, "list_slice")?;
    let end = number(&mut args, 2, "list_slice")?;
    let len = l.len();
    let start = slice_bound(start, len);
    let end = slice_bound(end, len);
    Ok(Value::list(if start >= end {
        Vec::new()
    } else {
        l[start..end].to_vec()
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// map
// ─────────────────────────────────────────────────────────────────────────

fn map_arg(
    args: &mut [Value],
    index: usize,
    op: &'static str,
) -> Result<Arc<crate::value::MapValue>, VmError> {
    match arg(args, index) {
        Value::Map(m) => Ok(m),
        other => Err(type_error("map", &other, op)),
    }
}

// Uniform native signature; the wrap is the calling convention.
#[allow(clippy::unnecessary_wraps)]
fn map_empty(_args: Vec<Value>) -> Result<Value, VmError> {
    Ok(Value::empty_map())
}

fn map_get(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_get")?;
    let key = arg(&mut args, 1);
    Ok(m.get(&key).cloned().map_or_else(Value::none, Value::some))
}

fn map_insert(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_insert")?;
    let key = arg(&mut args, 1);
    let value = arg(&mut args, 2);
    Ok(Value::Map(Arc::new(m.insert(key, value))))
}

fn map_remove(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_remove")?;
    let key = arg(&mut args, 1);
    Ok(Value::Map(Arc::new(m.remove(&key))))
}

fn map_contains(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_contains")?;
    let key = arg(&mut args, 1);
    Ok(Value::Bool(m.contains_key(&key)))
}

fn map_length(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_length")?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Value::Number(m.len() as f64))
}

fn map_keys(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_keys")?;
    Ok(Value::list(m.keys()))
}

fn map_values(mut args: Vec<Value>) -> Result<Value, VmError> {
    let m = map_arg(&mut args, 0, "map_values")?;
    Ok(Value::list(m.values()))
}

// ─────────────────────────────────────────────────────────────────────────
// set
// ─────────────────────────────────────────────────────────────────────────

fn set_arg(
    args: &mut [Value],
    index: usize,
    op: &'static str,
) -> Result<Arc<crate::value::SetValue>, VmError> {
    match arg(args, index) {
        Value::Set(s) => Ok(s),
        other => Err(type_error("set", &other, op)),
    }
}

// Uniform native signature; the wrap is the calling convention.
#[allow(clippy::unnecessary_wraps)]
fn set_empty(_args: Vec<Value>) -> Result<Value, VmError> {
    Ok(Value::empty_set())
}

fn set_insert(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = set_arg(&mut args, 0, "set_insert")?;
    let value = arg(&mut args, 1);
    Ok(Value::Set(Arc::new(s.insert(value))))
}

fn set_remove(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = set_arg(&mut args, 0, "set_remove")?;
    let value = arg(&mut args, 1);
    Ok(Value::Set(Arc::new(s.remove(&value))))
}

fn set_contains(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = set_arg(&mut args, 0, "set_contains")?;
    let value = arg(&mut args, 1);
    Ok(Value::Bool(s.contains(&value)))
}

fn set_length(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = set_arg(&mut args, 0, "set_length")?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Value::Number(s.len() as f64))
}

fn set_union(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = set_arg(&mut args, 0, "set_union")?;
    let b = set_arg(&mut args, 1, "set_union")?;
    Ok(Value::Set(Arc::new(a.union(&b))))
}

fn set_intersection(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = set_arg(&mut args, 0, "set_intersection")?;
    let b = set_arg(&mut args, 1, "set_intersection")?;
    Ok(Value::Set(Arc::new(a.intersection(&b))))
}

fn set_difference(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = set_arg(&mut args, 0, "set_difference")?;
    let b = set_arg(&mut args, 1, "set_difference")?;
    Ok(Value::Set(Arc::new(a.difference(&b))))
}

fn set_to_list(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = set_arg(&mut args, 0, "set_to_list")?;
    Ok(Value::list(s.to_list()))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "collections", "list"]);
    bind(reg, &m, "length", 0x0301, 1, list_length);
    bind(reg, &m, "get", 0x0302, 2, list_get);
    bind(reg, &m, "head", 0x0303, 1, list_head);
    bind(reg, &m, "tail", 0x0304, 1, list_tail);
    bind(reg, &m, "concat", 0x0305, 2, list_concat);
    bind(reg, &m, "append", 0x0306, 2, list_append);
    bind(reg, &m, "is_empty", 0x0307, 1, list_is_empty);
    bind(reg, &m, "last", 0x0308, 1, list_last);
    bind(reg, &m, "reverse", 0x0309, 1, list_reverse);
    bind(reg, &m, "sort", 0x030A, 1, list_sort);
    bind(reg, &m, "slice", 0x030B, 3, list_slice);

    let m = module(&["core", "collections", "map"]);
    bind(reg, &m, "empty", 0x0401, 0, map_empty);
    bind(reg, &m, "get", 0x0402, 2, map_get);
    bind(reg, &m, "insert", 0x0403, 3, map_insert);
    bind(reg, &m, "remove", 0x0404, 2, map_remove);
    bind(reg, &m, "contains", 0x0405, 2, map_contains);
    bind(reg, &m, "length", 0x0406, 1, map_length);
    bind(reg, &m, "keys", 0x0407, 1, map_keys);
    bind(reg, &m, "values", 0x0408, 1, map_values);

    let m = module(&["core", "collections", "set"]);
    bind(reg, &m, "empty", 0x0501, 0, set_empty);
    bind(reg, &m, "insert", 0x0502, 2, set_insert);
    bind(reg, &m, "remove", 0x0503, 2, set_remove);
    bind(reg, &m, "contains", 0x0504, 2, set_contains);
    bind(reg, &m, "length", 0x0505, 1, set_length);
    bind(reg, &m, "union", 0x0506, 2, set_union);
    bind(reg, &m, "intersection", 0x0507, 2, set_intersection);
    bind(reg, &m, "difference", 0x0508, 2, set_difference);
    bind(reg, &m, "to_list", 0x0509, 1, set_to_list);
}
