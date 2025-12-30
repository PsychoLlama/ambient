//! Type serialization to/from JSON for database storage.
//!
//! This module provides compact JSON serialization for types, optimized for:
//! - Fast read/write to `SQLite`
//! - Human-readable for debugging
//! - Round-trip fidelity (deserialize(serialize(t)) == t)

#![allow(
    clippy::cast_possible_truncation,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines
)]

use serde_json::{json, Value};
use std::sync::Arc;

use crate::types::{
    AbilityId, AbilitySet, AbilityValueType, AbilityVarId, ForallType, FunctionType, HandlerType,
    NamedType, NominalType, RecordType, Type, TypeVar, TypeVarId,
};

/// Serialize a type to JSON string.
#[must_use]
pub fn serialize_type(ty: &Type) -> String {
    serialize_type_value(ty).to_string()
}

/// Serialize a type to JSON value.
#[must_use]
pub fn serialize_type_value(ty: &Type) -> Value {
    match ty {
        // Primitives
        Type::Unit => json!({"t": "unit"}),
        Type::Bool => json!({"t": "bool"}),
        Type::Number => json!({"t": "number"}),
        Type::String => json!({"t": "string"}),
        Type::Bytes => json!({"t": "bytes"}),
        Type::Never => json!({"t": "never"}),
        Type::Error => json!({"t": "error"}),
        Type::Hole => json!({"t": "hole"}),

        // Type variable
        Type::Var(TypeVar::Unbound(id)) => json!({"t": "var", "id": id}),
        Type::Var(TypeVar::Link(link)) => serialize_type_value(&link.borrow()),

        // Tuple
        Type::Tuple(elems) => json!({
            "t": "tuple",
            "elems": elems.iter().map(serialize_type_value).collect::<Vec<_>>()
        }),

        // Record
        Type::Record(rec) => json!({
            "t": "record",
            "fields": rec.fields.iter()
                .map(|(name, ty)| json!([name.as_ref(), serialize_type_value(ty)]))
                .collect::<Vec<_>>()
        }),

        // Function
        Type::Function(func) => json!({
            "t": "function",
            "params": func.params.iter().map(serialize_type_value).collect::<Vec<_>>(),
            "ret": serialize_type_value(&func.ret),
            "abilities": serialize_ability_set_value(&func.abilities)
        }),

        // Named type (List<T>, Option<T>, etc.)
        Type::Named(named) => json!({
            "t": "named",
            "name": named.name.as_ref(),
            "args": named.args.iter().map(serialize_type_value).collect::<Vec<_>>()
        }),

        // Nominal type
        Type::Nominal(nom) => json!({
            "t": "nominal",
            "uuid": nom.uuid.to_string(),
            "inner": serialize_type_value(&nom.inner),
            "name": nom.name.as_ref().map(|n| n.as_ref())
        }),

        // Forall (polymorphic)
        Type::Forall(forall) => json!({
            "t": "forall",
            "vars": forall.vars,
            "ability_vars": forall.ability_vars,
            "body": serialize_type_value(&forall.body)
        }),

        // Ability value
        Type::AbilityValue(av) => json!({
            "t": "ability_value",
            "result": serialize_type_value(&av.result),
            "ability": serialize_ability_set_value(&av.ability)
        }),

        // Handler type
        Type::Handler(handler) => json!({
            "t": "handler",
            "ability": handler.ability
        }),
    }
}

/// Serialize an ability set to JSON string.
#[must_use]
pub fn serialize_ability_set(abilities: &AbilitySet) -> String {
    serialize_ability_set_value(abilities).to_string()
}

/// Serialize an ability set to JSON value.
#[must_use]
pub fn serialize_ability_set_value(abilities: &AbilitySet) -> Value {
    match abilities {
        AbilitySet::Empty => json!({"kind": "empty"}),
        AbilitySet::Concrete(ids) => json!({"kind": "concrete", "abilities": ids}),
        AbilitySet::Var(id) => json!({"kind": "var", "id": id}),
        AbilitySet::Row { concrete, tail } => json!({
            "kind": "row",
            "concrete": concrete,
            "tail": tail
        }),
    }
}

/// Error type for deserialization.
#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid type format: {0}")]
    InvalidFormat(String),
    #[error("Unknown type tag: {0}")]
    UnknownTag(String),
    #[error("Invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
}

/// Deserialize a type from JSON string.
///
/// # Errors
/// Returns an error if the JSON is invalid or doesn't represent a valid type.
pub fn deserialize_type(json: &str) -> Result<Type, DeserializeError> {
    let value: Value = serde_json::from_str(json)?;
    deserialize_type_value(&value)
}

/// Deserialize a type from JSON value.
///
/// # Errors
/// Returns an error if the value doesn't represent a valid type.
pub fn deserialize_type_value(value: &Value) -> Result<Type, DeserializeError> {
    let tag = value
        .get("t")
        .and_then(Value::as_str)
        .ok_or_else(|| DeserializeError::InvalidFormat("missing 't' field".to_string()))?;

    match tag {
        // Primitives
        "unit" => Ok(Type::Unit),
        "bool" => Ok(Type::Bool),
        "number" => Ok(Type::Number),
        "string" => Ok(Type::String),
        "never" => Ok(Type::Never),
        "error" => Ok(Type::Error),
        "hole" => Ok(Type::Hole),

        // Type variable
        "var" => {
            let id = value.get("id").and_then(Value::as_u64).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'id' for var".to_string())
            })? as TypeVarId;
            Ok(Type::var(id))
        }

        // Tuple
        "tuple" => {
            let elems = value
                .get("elems")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'elems' for tuple".to_string())
                })?;
            let elems: Result<Vec<_>, _> = elems.iter().map(deserialize_type_value).collect();
            Ok(Type::Tuple(elems?))
        }

        // Record
        "record" => {
            let fields = value
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'fields' for record".to_string())
                })?;
            let fields: Result<Vec<_>, _> = fields
                .iter()
                .map(|f| {
                    let arr = f.as_array().ok_or_else(|| {
                        DeserializeError::InvalidFormat("field must be array".to_string())
                    })?;
                    if arr.len() != 2 {
                        return Err(DeserializeError::InvalidFormat(
                            "field must be [name, type]".to_string(),
                        ));
                    }
                    let name = arr[0].as_str().ok_or_else(|| {
                        DeserializeError::InvalidFormat("field name must be string".to_string())
                    })?;
                    let ty = deserialize_type_value(&arr[1])?;
                    Ok((Arc::from(name), ty))
                })
                .collect();
            Ok(Type::Record(RecordType::new(fields?)))
        }

        // Function
        "function" => {
            let params = value
                .get("params")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'params' for function".to_string())
                })?;
            let params: Result<Vec<_>, _> = params.iter().map(deserialize_type_value).collect();
            let ret = value.get("ret").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'ret' for function".to_string())
            })?;
            let abilities = value.get("abilities").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'abilities' for function".to_string())
            })?;
            Ok(Type::Function(FunctionType::with_abilities(
                params?,
                deserialize_type_value(ret)?,
                deserialize_ability_set_value(abilities)?,
            )))
        }

        // Named type
        "named" => {
            let name = value.get("name").and_then(Value::as_str).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'name' for named type".to_string())
            })?;
            let args = value.get("args").and_then(Value::as_array).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'args' for named type".to_string())
            })?;
            let args: Result<Vec<_>, _> = args.iter().map(deserialize_type_value).collect();
            Ok(Type::Named(NamedType::new(name, args?)))
        }

        // Nominal type
        "nominal" => {
            let uuid_str = value.get("uuid").and_then(Value::as_str).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'uuid' for nominal type".to_string())
            })?;
            let uuid = uuid::Uuid::parse_str(uuid_str)?;
            let inner = value.get("inner").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'inner' for nominal type".to_string())
            })?;
            let name = value.get("name").and_then(Value::as_str).map(Arc::from);
            Ok(Type::Nominal(NominalType::new(
                uuid,
                deserialize_type_value(inner)?,
                name,
            )))
        }

        // Forall
        "forall" => {
            let vars = value.get("vars").and_then(Value::as_array).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'vars' for forall".to_string())
            })?;
            let vars: Vec<TypeVarId> = vars
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as TypeVarId)
                .collect();
            let ability_vars: Vec<AbilityVarId> = value
                .get("ability_vars")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_u64)
                        .map(|v| v as AbilityVarId)
                        .collect()
                })
                .unwrap_or_default();
            let body = value.get("body").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'body' for forall".to_string())
            })?;
            Ok(Type::Forall(ForallType::with_abilities(
                vars,
                ability_vars,
                deserialize_type_value(body)?,
            )))
        }

        // Ability value
        "ability_value" => {
            let result = value.get("result").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'result' for ability_value".to_string())
            })?;
            let ability = value.get("ability").ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'ability' for ability_value".to_string())
            })?;
            Ok(Type::AbilityValue(AbilityValueType::new(
                deserialize_type_value(result)?,
                deserialize_ability_set_value(ability)?,
            )))
        }

        // Handler
        "handler" => {
            let ability = value
                .get("ability")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'ability' for handler".to_string())
                })? as AbilityId;
            Ok(Type::Handler(HandlerType::new(ability)))
        }

        unknown => Err(DeserializeError::UnknownTag(unknown.to_string())),
    }
}

/// Deserialize an ability set from JSON string.
///
/// # Errors
/// Returns an error if the JSON is invalid or doesn't represent a valid ability set.
pub fn deserialize_ability_set(json: &str) -> Result<AbilitySet, DeserializeError> {
    let value: Value = serde_json::from_str(json)?;
    deserialize_ability_set_value(&value)
}

/// Deserialize an ability set from JSON value.
///
/// # Errors
/// Returns an error if the value doesn't represent a valid ability set.
pub fn deserialize_ability_set_value(value: &Value) -> Result<AbilitySet, DeserializeError> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| DeserializeError::InvalidFormat("missing 'kind' field".to_string()))?;

    match kind {
        "empty" => Ok(AbilitySet::Empty),
        "concrete" => {
            let abilities = value
                .get("abilities")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'abilities' for concrete".to_string())
                })?;
            let abilities: Vec<AbilityId> = abilities
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as AbilityId)
                .collect();
            Ok(AbilitySet::from_abilities(abilities))
        }
        "var" => {
            let id = value.get("id").and_then(Value::as_u64).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'id' for ability var".to_string())
            })? as AbilityVarId;
            Ok(AbilitySet::Var(id))
        }
        "row" => {
            let concrete = value
                .get("concrete")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DeserializeError::InvalidFormat("missing 'concrete' for row".to_string())
                })?;
            let concrete: Vec<AbilityId> = concrete
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as AbilityId)
                .collect();
            let tail = value.get("tail").and_then(Value::as_u64).ok_or_else(|| {
                DeserializeError::InvalidFormat("missing 'tail' for row".to_string())
            })? as AbilityVarId;
            Ok(AbilitySet::Row { concrete, tail })
        }
        unknown => Err(DeserializeError::InvalidFormat(format!(
            "unknown ability kind: {unknown}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_round_trip() {
        let types = vec![
            Type::Unit,
            Type::Bool,
            Type::Number,
            Type::String,
            Type::Never,
        ];
        for ty in types {
            let json = serialize_type(&ty);
            let result = deserialize_type(&json).expect("deserialize failed");
            assert_eq!(ty, result);
        }
    }

    #[test]
    fn test_tuple_round_trip() {
        let ty = Type::Tuple(vec![Type::Number, Type::String]);
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_record_round_trip() {
        let ty = Type::record([("x", Type::Number), ("y", Type::String)]);
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_function_round_trip() {
        let ty = Type::function_with_abilities(
            vec![Type::Number, Type::String],
            Type::Bool,
            AbilitySet::from_abilities([1, 2]),
        );
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_named_type_round_trip() {
        let ty = Type::named("List", vec![Type::Number]);
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_forall_round_trip() {
        let ty = Type::Forall(ForallType::with_abilities(
            vec![0, 1],
            vec![2],
            Type::function_with_abilities(vec![Type::var(0)], Type::var(1), AbilitySet::Var(2)),
        ));
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_ability_set_round_trip() {
        let sets = vec![
            AbilitySet::Empty,
            AbilitySet::from_abilities([1, 2, 3]),
            AbilitySet::Var(42),
            AbilitySet::Row {
                concrete: vec![1, 2],
                tail: 99,
            },
        ];
        for set in sets {
            let json = serialize_ability_set(&set);
            let result = deserialize_ability_set(&json).expect("deserialize failed");
            assert_eq!(set, result);
        }
    }

    #[test]
    fn test_nominal_round_trip() {
        let uuid = uuid::Uuid::new_v4();
        let ty = Type::nominal(uuid, Type::String, Some("UserId"));
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_handler_round_trip() {
        let ty = Type::handler(42);
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }

    #[test]
    fn test_ability_value_round_trip() {
        let ty = Type::ability_value(Type::String, AbilitySet::from_abilities([1, 2]));
        let json = serialize_type(&ty);
        let result = deserialize_type(&json).expect("deserialize failed");
        assert_eq!(ty, result);
    }
}
