//! Module serialization and deserialization.
//!
//! This provides JSON-based serialization for compiled Ambient modules.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use ambient_engine::bytecode::CompiledFunction;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::value::Value;

/// Serialized module format.
#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedModule {
    pub functions: Vec<SerializedFunction>,
    pub function_names: Vec<(String, String)>, // (name, hash_hex)
    pub entry_point: Option<String>,           // hash_hex
}

/// Serialized function format.
#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedFunction {
    pub hash: String, // hex
    pub bytecode: Vec<u8>,
    pub constants: Vec<SerializedValue>,
    pub local_count: u16,
    pub param_count: u8,
    pub dependencies: Vec<String>, // hex hashes
}

/// Serialized value format.
#[derive(Debug, Serialize, Deserialize)]
pub enum SerializedValue {
    Unit,
    Bool(bool),
    Number(f64),
    String(String),
    Bytes(Vec<u8>),
    FunctionRef(String), // hex hash
                         // Tuples and records not typically in constant pools
}

/// Serialize a compiled module to a serializable format.
pub fn serialize_module(module: &CompiledModule) -> SerializedModule {
    SerializedModule {
        functions: module
            .functions
            .values()
            .map(|f| SerializedFunction {
                hash: f.hash.to_hex().to_string(),
                bytecode: f.bytecode.clone(),
                constants: f.constants.iter().map(serialize_value).collect(),
                local_count: f.local_count,
                param_count: f.param_count,
                dependencies: f
                    .dependencies
                    .iter()
                    .map(|h| h.to_hex().to_string())
                    .collect(),
            })
            .collect(),
        function_names: module
            .function_names
            .iter()
            .map(|(name, hash)| (name.to_string(), hash.to_hex().to_string()))
            .collect(),
        entry_point: module.entry_point.map(|h| h.to_hex().to_string()),
    }
}

fn serialize_value(value: &Value) -> SerializedValue {
    match value {
        Value::Unit => SerializedValue::Unit,
        Value::Bool(b) => SerializedValue::Bool(*b),
        Value::Number(n) => SerializedValue::Number(*n),
        Value::String(s) => SerializedValue::String((**s).clone()),
        Value::Bytes(b) => SerializedValue::Bytes((**b).clone()),
        Value::FunctionRef(h) => SerializedValue::FunctionRef(h.to_hex().to_string()),
        // These shouldn't appear in constant pools
        Value::Tuple(_)
        | Value::Record(_)
        | Value::SuspendedAbility(_)
        | Value::Continuation(_)
        | Value::Closure(_)
        | Value::Handler(_)
        | Value::List(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::Enum(_)
        | Value::Module(_)
        | Value::ModuleMember(_) => SerializedValue::Unit,
    }
}

/// Deserialize a module from a serializable format.
pub fn deserialize_module(serialized: &SerializedModule) -> Result<CompiledModule> {
    let mut functions = HashMap::new();
    let mut function_names = HashMap::new();

    for sf in &serialized.functions {
        let hash = blake3::Hash::from_hex(&sf.hash).context("invalid hash")?;
        let constants: Vec<Value> = sf
            .constants
            .iter()
            .map(deserialize_value)
            .collect::<Result<_>>()?;
        let dependencies: Vec<blake3::Hash> = sf
            .dependencies
            .iter()
            .map(|h| blake3::Hash::from_hex(h).context("invalid dependency hash"))
            .collect::<Result<_>>()?;

        // Create function with the stored hash (we can't recompute since it depends on content)
        let func = CompiledFunction {
            hash,
            bytecode: sf.bytecode.clone(),
            constants,
            local_count: sf.local_count,
            param_count: sf.param_count,
            dependencies,
            debug_info: None, // Debug info not serialized yet
        };
        functions.insert(hash, func);
    }

    for (name, hash_str) in &serialized.function_names {
        let hash = blake3::Hash::from_hex(hash_str).context("invalid hash")?;
        function_names.insert(Arc::from(name.as_str()), hash);
    }

    let entry_point = serialized
        .entry_point
        .as_ref()
        .map(|h| blake3::Hash::from_hex(h).context("invalid entry point hash"))
        .transpose()?;

    Ok(CompiledModule {
        functions,
        function_names,
        lambda_parents: HashMap::new(), // Lambdas not serialized
        entry_point,
    })
}

fn deserialize_value(sv: &SerializedValue) -> Result<Value> {
    Ok(match sv {
        SerializedValue::Unit => Value::Unit,
        SerializedValue::Bool(b) => Value::Bool(*b),
        SerializedValue::Number(n) => Value::Number(*n),
        SerializedValue::String(s) => Value::String(Arc::new(s.clone())),
        SerializedValue::Bytes(b) => Value::bytes(b.clone()),
        SerializedValue::FunctionRef(h) => {
            Value::FunctionRef(blake3::Hash::from_hex(h).context("invalid function ref hash")?)
        }
    })
}
