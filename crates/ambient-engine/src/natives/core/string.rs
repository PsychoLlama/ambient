//! Natives for `core::primitives::string`: the UTF-8 string surface.

use crate::{Value, VmError};

use super::{NativeRegistry, bind, list, module, number, slice_bound, string};

fn length(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_length")?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Value::Number(s.len() as f64))
}

fn concat(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = string(&mut args, 0, "string_concat")?;
    let b = string(&mut args, 1, "string_concat")?;
    let mut result = (*a).clone();
    result.push_str(&b);
    Ok(Value::string(result))
}

fn contains(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_contains")?;
    let needle = string(&mut args, 1, "string_contains")?;
    Ok(Value::Bool(s.contains(&*needle)))
}

fn split(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_split")?;
    let sep = string(&mut args, 1, "string_split")?;
    let parts: Vec<Value> = s
        .split(&*sep)
        .map(|part| Value::string(part.to_string()))
        .collect();
    Ok(Value::list(parts))
}

fn join(mut args: Vec<Value>) -> Result<Value, VmError> {
    let parts = list(&mut args, 0, "string_join")?;
    let sep = string(&mut args, 1, "string_join")?;
    let parts: Vec<String> = parts
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some((**s).clone()),
            _ => None,
        })
        .collect();
    Ok(Value::string(parts.join(&*sep)))
}

fn trim(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_trim")?;
    Ok(Value::string(s.trim().to_string()))
}

fn slice(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_slice")?;
    let start = number(&mut args, 1, "string_slice")?;
    let end = number(&mut args, 2, "string_slice")?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let start = slice_bound(start, len);
    let end = slice_bound(end, len);
    let result: String = if start >= end {
        String::new()
    } else {
        chars[start..end].iter().collect()
    };
    Ok(Value::string(result))
}

fn chars(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_chars")?;
    let chars: Vec<Value> = s.chars().map(|c| Value::string(c.to_string())).collect();
    Ok(Value::list(chars))
}

fn replace(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_replace")?;
    let from = string(&mut args, 1, "string_replace")?;
    let to = string(&mut args, 2, "string_replace")?;
    Ok(Value::string(s.replace(&*from, &to)))
}

fn starts_with(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_starts_with")?;
    let prefix = string(&mut args, 1, "string_starts_with")?;
    Ok(Value::Bool(s.starts_with(&*prefix)))
}

fn ends_with(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_ends_with")?;
    let suffix = string(&mut args, 1, "string_ends_with")?;
    Ok(Value::Bool(s.ends_with(&*suffix)))
}

fn to_upper(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_to_upper")?;
    Ok(Value::string(s.to_uppercase()))
}

fn to_lower(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_to_lower")?;
    Ok(Value::string(s.to_lowercase()))
}

fn index_of(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_index_of")?;
    let needle = string(&mut args, 1, "string_index_of")?;
    Ok(match s.find(&*needle) {
        Some(idx) => {
            // Convert byte index to character index.
            #[allow(clippy::cast_precision_loss)]
            let char_idx = s[..idx].chars().count() as f64;
            Value::some(Value::Number(char_idx))
        }
        None => Value::none(),
    })
}

fn repeat(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_repeat")?;
    let count = number(&mut args, 1, "string_repeat")?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(Value::string(s.repeat(count as usize)))
}

fn reverse(mut args: Vec<Value>) -> Result<Value, VmError> {
    let s = string(&mut args, 0, "string_reverse")?;
    let result: String = s.chars().rev().collect();
    Ok(Value::string(result))
}

fn compare(mut args: Vec<Value>) -> Result<Value, VmError> {
    let a = string(&mut args, 0, "string_compare")?;
    let b = string(&mut args, 1, "string_compare")?;
    let ordering = match a.cmp(&b) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    };
    Ok(Value::Number(ordering))
}

pub(super) fn register(reg: &mut NativeRegistry) {
    let m = module(&["core", "primitives", "string"]);
    bind(reg, &m, "length", 0x0201, 1, length);
    bind(reg, &m, "concat", 0x0202, 2, concat);
    bind(reg, &m, "contains", 0x0203, 2, contains);
    bind(reg, &m, "split", 0x0204, 2, split);
    bind(reg, &m, "join", 0x0205, 2, join);
    bind(reg, &m, "trim", 0x0206, 1, trim);
    bind(reg, &m, "slice", 0x0207, 3, slice);
    bind(reg, &m, "chars", 0x0208, 1, chars);
    bind(reg, &m, "replace", 0x0209, 3, replace);
    bind(reg, &m, "starts_with", 0x020A, 2, starts_with);
    bind(reg, &m, "ends_with", 0x020B, 2, ends_with);
    bind(reg, &m, "to_upper", 0x020C, 1, to_upper);
    bind(reg, &m, "to_lower", 0x020D, 1, to_lower);
    bind(reg, &m, "index_of", 0x020E, 2, index_of);
    bind(reg, &m, "repeat", 0x020F, 2, repeat);
    bind(reg, &m, "reverse", 0x0210, 1, reverse);
    bind(reg, &m, "compare", 0x0211, 2, compare);
}
