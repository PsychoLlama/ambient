//! Value serialization for code and data exchange between engines.
//!
//! This is the codec behind `core::protocol::serialize_value` /
//! `deserialize_value`: a bincode encoding of the wire-safe subset of
//! runtime values. Remote-execution *protocols* are written in Ambient
//! itself on top of the `Network` and `Execute` abilities (see
//! `examples/remote_server`); there is no Rust-side message layer.
//!
//! Function references cross as content hashes; handler values cross as
//! (ability hash, method-id → function-hash) tables, with the code itself
//! shipped separately in canonical packs. Values whose meaning cannot
//! survive the wire (closures with live environments, continuations,
//! maps/sets, modules) are a hard error, never silently degraded.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::value::Value;

// ─────────────────────────────────────────────────────────────────────────────
// Binary value serialization (for core::protocol intrinsics)
// ─────────────────────────────────────────────────────────────────────────────

/// Serializable representation of a Value for binary encoding.
///
/// This mirrors the runtime Value enum but uses standard Rust types
/// that can be serialized with serde/bincode.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum SerializableValue {
    Unit,
    Bool(bool),
    Number(f64),
    String(String),
    Binary(Vec<u8>),
    Tuple(Vec<SerializableValue>),
    List(Vec<SerializableValue>),
    Record(Vec<(String, SerializableValue)>),
    Enum {
        type_name: String,
        tag: u16,
        variant_name: String,
        payload: Option<Box<SerializableValue>>,
    },
    FunctionRef(String),
    AbilityRef(String),
    Handler {
        ability: String,
        methods: Vec<(u16, String)>,
        captures: Vec<SerializableValue>,
    },
}

impl SerializableValue {
    /// Convert a runtime value into wire form.
    ///
    /// # Errors
    ///
    /// Values whose meaning cannot survive the wire (closures with live
    /// environments, continuations, suspended abilities, maps/sets,
    /// modules) are a hard error, never silently degraded. Handler values
    /// cross by reference: their methods are content-addressed function
    /// hashes, so the receiver can fetch the code and reconstruct the
    /// handler exactly.
    pub fn try_from_value(value: &Value) -> Result<Self, &'static str> {
        Ok(match value {
            Value::Unit => SerializableValue::Unit,
            Value::Bool(b) => SerializableValue::Bool(*b),
            Value::Number(n) => SerializableValue::Number(*n),
            Value::String(s) => SerializableValue::String((**s).clone()),
            Value::Binary(b) => SerializableValue::Binary((**b).clone()),
            Value::Tuple(elements) => SerializableValue::Tuple(
                elements
                    .iter()
                    .map(Self::try_from_value)
                    .collect::<Result<_, _>>()?,
            ),
            Value::List(elements) => SerializableValue::List(
                elements
                    .iter()
                    .map(Self::try_from_value)
                    .collect::<Result<_, _>>()?,
            ),
            Value::Record(fields) => SerializableValue::Record(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.to_string(), Self::try_from_value(v)?)))
                    .collect::<Result<_, &'static str>>()?,
            ),
            Value::Enum(e) => SerializableValue::Enum {
                type_name: e.type_name.to_string(),
                tag: e.tag,
                variant_name: e.variant_name.to_string(),
                payload: e
                    .payload
                    .as_ref()
                    .map(|p| Self::try_from_value(p.as_ref()).map(Box::new))
                    .transpose()?,
            },
            Value::FunctionRef(hash) => SerializableValue::FunctionRef(hash.to_string()),
            Value::AbilityRef(id) => SerializableValue::AbilityRef(id.to_hex()),
            Value::Handler(h) => {
                let mut methods: Vec<(u16, String)> = h
                    .methods
                    .iter()
                    .map(|(id, hash)| (*id, hash.to_hex().to_string()))
                    .collect();
                methods.sort_by_key(|(id, _)| *id);
                SerializableValue::Handler {
                    ability: h.ability_id.to_hex(),
                    methods,
                    captures: h
                        .captures
                        .iter()
                        .map(Self::try_from_value)
                        .collect::<Result<_, _>>()?,
                }
            }
            Value::ObjectRef(_) => return Err("object reference"),
            Value::Closure(_) => return Err("closure"),
            Value::SuspendedAbility(_) => return Err("suspended ability"),
            Value::Continuation(_) => return Err("continuation"),
            Value::Map(_) => return Err("map"),
            Value::Set(_) => return Err("set"),
            Value::Module(_) => return Err("module"),
            Value::ModuleMember(_) => return Err("module member"),
        })
    }
}

impl From<SerializableValue> for Value {
    fn from(value: SerializableValue) -> Self {
        match value {
            SerializableValue::Unit => Value::Unit,
            SerializableValue::Bool(b) => Value::Bool(b),
            SerializableValue::Number(n) => Value::Number(n),
            SerializableValue::String(s) => Value::String(Arc::new(s)),
            SerializableValue::Binary(b) => Value::binary(b),
            SerializableValue::Tuple(elements) => {
                Value::tuple(elements.into_iter().map(Value::from).collect())
            }
            SerializableValue::List(elements) => {
                Value::list(elements.into_iter().map(Value::from).collect())
            }
            SerializableValue::Record(fields) => {
                let pairs: Vec<(Arc<str>, Value)> = fields
                    .into_iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), Value::from(v)))
                    .collect();
                Value::record(pairs)
            }
            SerializableValue::Enum {
                type_name,
                tag,
                variant_name,
                payload,
            } => Value::enum_variant(
                type_name.as_str(),
                tag,
                variant_name.as_str(),
                payload.map(|p| Value::from(*p)),
            ),
            SerializableValue::FunctionRef(hash) => {
                // Parse the hash, falling back to an all-zeros hash if invalid
                let parsed = blake3::Hash::from_hex(&hash)
                    .unwrap_or_else(|_| blake3::Hash::from_bytes([0u8; 32]));
                Value::FunctionRef(parsed)
            }
            SerializableValue::AbilityRef(hex) => {
                // Parse the hash, falling back to an all-zeros identity if invalid
                let parsed = ambient_core::AbilityId::from_hex(&hex)
                    .unwrap_or_else(|| ambient_core::AbilityId::from_bytes([0u8; 32]));
                Value::AbilityRef(parsed)
            }
            SerializableValue::Handler {
                ability,
                methods,
                captures,
            } => {
                let ability = ambient_core::AbilityId::from_hex(&ability)
                    .unwrap_or_else(|| ambient_core::AbilityId::from_bytes([0u8; 32]));
                let methods = methods
                    .into_iter()
                    .filter_map(|(id, hex)| {
                        blake3::Hash::from_hex(&hex).ok().map(|hash| (id, hash))
                    })
                    .collect();
                let captures = captures.into_iter().map(Value::from).collect();
                Value::Handler(std::sync::Arc::new(
                    ambient_ability::HandlerValue::with_captures(ability, methods, captures),
                ))
            }
        }
    }
}

/// Serialize a Value to bytes using bincode.
///
/// # Errors
///
/// Returns the offending kind's name when the value cannot cross the wire
/// (see [`SerializableValue::try_from_value`]).
pub fn serialize_value(value: &Value) -> Result<Vec<u8>, &'static str> {
    let serializable = SerializableValue::try_from_value(value)?;
    Ok(bincode::serialize(&serializable).unwrap_or_default())
}

/// Deserialize bytes to a Value using bincode.
#[must_use]
pub fn deserialize_value(bytes: &[u8]) -> Option<Value> {
    let serializable: SerializableValue = bincode::deserialize(bytes).ok()?;
    Some(Value::from(serializable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_unit() {
        let value = Value::Unit;
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        assert!(matches!(result, Value::Unit));
    }

    #[test]
    fn test_serialize_bool() {
        let value = Value::Bool(true);
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        assert!(matches!(result, Value::Bool(true)));
    }

    #[test]
    fn test_serialize_number() {
        let value = Value::Number(42.5);
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        match result {
            Value::Number(n) => assert!((n - 42.5).abs() < f64::EPSILON),
            _ => panic!("expected number"),
        }
    }

    #[test]
    fn test_serialize_string() {
        let value = Value::string("hello".to_string());
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        match result {
            Value::String(s) => assert_eq!(&*s, "hello"),
            _ => panic!("expected string"),
        }
    }

    #[test]
    fn test_serialize_list() {
        let value = Value::list(vec![Value::Number(1.0), Value::Number(2.0)]);
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        match result {
            Value::List(elements) => {
                assert_eq!(elements.len(), 2);
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn test_serialize_option_some() {
        let value = Value::some(Value::Number(42.0));
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        match result {
            Value::Enum(e) => {
                assert_eq!(&*e.type_name, "Option");
                assert_eq!(e.tag, 1); // Some
            }
            _ => panic!("expected enum"),
        }
    }

    #[test]
    fn test_serialize_option_none() {
        let value = Value::none();
        let bytes = serialize_value(&value).expect("serializable");
        let result = deserialize_value(&bytes).unwrap();
        match result {
            Value::Enum(e) => {
                assert_eq!(&*e.type_name, "Option");
                assert_eq!(e.tag, 0); // None
            }
            _ => panic!("expected enum"),
        }
    }
}
