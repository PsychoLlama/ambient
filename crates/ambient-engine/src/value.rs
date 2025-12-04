use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Represents a runtime value in the language.
///
/// Values are immutable and use reference counting for efficient sharing of
/// heap-allocated data (strings, tuples, records).
///
/// Most values are serializable for remote execution and storage.
/// `Continuation` is not serializable as it contains runtime-specific state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    /// Unit type, represents absence of a meaningful value.
    Unit,

    /// Boolean value.
    Bool(bool),

    /// 64-bit floating point number (the only numeric type per spec).
    Number(f64),

    /// UTF-8 string.
    String(Arc<String>),

    /// Tuple: fixed-size, heterogeneous collection accessed by index.
    Tuple(Arc<Vec<Value>>),

    /// Record: named fields with values, structural typing.
    Record(Arc<HashMap<Arc<str>, Value>>),

    /// Reference to a content-addressed function.
    FunctionRef(blake3::Hash),

    /// A suspended ability operation that can be performed later.
    ///
    /// Contains the ability ID, method ID, and arguments.
    SuspendedAbility(Arc<SuspendedAbility>),

    /// A captured continuation that can be resumed (single-shot).
    ///
    /// Note: Continuations are NOT serializable as they contain runtime state.
    /// Attempting to serialize a Continuation will produce an error.
    #[serde(skip)]
    Continuation(Arc<Continuation>),

    /// A closure: a function with captured environment.
    ///
    /// Contains the function hash and the captured values (environment).
    Closure(Arc<Closure>),

    /// A first-class handler value that can handle an ability.
    ///
    /// Handler values can be passed around, stored, composed with other handlers,
    /// and used in `handle ... with handler_value` expressions.
    Handler(Arc<HandlerValue>),

    /// A list: variable-length, homogeneous collection.
    /// `List<T>`
    List(Arc<Vec<Value>>),

    /// A map: key-value collection with string keys.
    /// `Map<K, V>` where K is always string for now (simplifies hashing).
    Map(Arc<MapValue>),

    /// A set: collection of unique values.
    /// `Set<T>` - elements are compared by value equality.
    Set(Arc<SetValue>),
}

/// A map value with string keys.
///
/// Uses a `BTreeMap` internally for deterministic ordering during
/// serialization and equality comparisons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MapValue {
    /// The key-value pairs, stored in a sorted map for deterministic ordering.
    pub entries: std::collections::BTreeMap<Arc<str>, Value>,
}

impl MapValue {
    /// Create a new empty map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
        }
    }

    /// Create a map from an iterator of key-value pairs.
    pub fn from_entries(iter: impl IntoIterator<Item = (impl Into<Arc<str>>, Value)>) -> Self {
        Self {
            entries: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }

    /// Get a value by key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.get(key)
    }

    /// Insert a key-value pair, returning a new map.
    #[must_use]
    pub fn insert(&self, key: impl Into<Arc<str>>, value: Value) -> Self {
        let mut entries = self.entries.clone();
        entries.insert(key.into(), value);
        Self { entries }
    }

    /// Remove a key, returning a new map.
    #[must_use]
    pub fn remove(&self, key: &str) -> Self {
        let mut entries = self.entries.clone();
        entries.remove(key);
        Self { entries }
    }

    /// Check if the map contains a key.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Get the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all keys as a list.
    #[must_use]
    pub fn keys(&self) -> Vec<Arc<str>> {
        self.entries.keys().cloned().collect()
    }

    /// Get all values as a list.
    #[must_use]
    pub fn values(&self) -> Vec<Value> {
        self.entries.values().cloned().collect()
    }
}

impl Default for MapValue {
    fn default() -> Self {
        Self::new()
    }
}

/// A set value containing unique elements.
///
/// Uses a `Vec` internally but maintains set semantics (no duplicates).
/// Elements are compared using `Value::eq`. Order is preserved for
/// deterministic serialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetValue {
    /// The elements in the set. No duplicates (checked on insert).
    pub elements: Vec<Value>,
}

impl SetValue {
    /// Create a new empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
        }
    }

    /// Create a set from an iterator of values. Duplicates are ignored.
    pub fn from_values(iter: impl IntoIterator<Item = Value>) -> Self {
        let mut elements = Vec::new();
        for value in iter {
            if !elements.contains(&value) {
                elements.push(value);
            }
        }
        Self { elements }
    }

    /// Check if the set contains a value.
    #[must_use]
    pub fn contains(&self, value: &Value) -> bool {
        self.elements.contains(value)
    }

    /// Insert a value, returning a new set.
    #[must_use]
    pub fn insert(&self, value: Value) -> Self {
        if self.contains(&value) {
            self.clone()
        } else {
            let mut elements = self.elements.clone();
            elements.push(value);
            Self { elements }
        }
    }

    /// Remove a value, returning a new set.
    #[must_use]
    pub fn remove(&self, value: &Value) -> Self {
        let elements: Vec<_> = self
            .elements
            .iter()
            .filter(|v| *v != value)
            .cloned()
            .collect();
        Self { elements }
    }

    /// Get the number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Check if the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Compute the union with another set, returning a new set.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for value in &other.elements {
            if !result.contains(value) {
                result.elements.push(value.clone());
            }
        }
        result
    }

    /// Compute the intersection with another set, returning a new set.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let elements: Vec<_> = self
            .elements
            .iter()
            .filter(|v| other.contains(v))
            .cloned()
            .collect();
        Self { elements }
    }

    /// Compute the difference with another set (self - other), returning a new set.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        let elements: Vec<_> = self
            .elements
            .iter()
            .filter(|v| !other.contains(v))
            .cloned()
            .collect();
        Self { elements }
    }

    /// Convert the set to a list.
    #[must_use]
    pub fn to_list(&self) -> Vec<Value> {
        self.elements.clone()
    }
}

impl Default for SetValue {
    fn default() -> Self {
        Self::new()
    }
}

/// A suspended ability operation waiting to be performed.
///
/// This type is fully serializable, allowing ability values to be stored,
/// transmitted, and executed remotely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendedAbility {
    /// The ability being invoked (e.g., "Filesystem", "Console").
    pub ability_id: u16,

    /// The method being called on the ability (e.g., "read", "print").
    pub method_id: u16,

    /// The arguments to pass to the ability method.
    pub args: Vec<Value>,
}

impl SuspendedAbility {
    /// Create a new suspended ability.
    #[must_use]
    pub fn new(ability_id: u16, method_id: u16, args: Vec<Value>) -> Self {
        Self {
            ability_id,
            method_id,
            args,
        }
    }
}

/// A closure combining a function with its captured environment.
///
/// Closures are created when lambda expressions capture variables from
/// their surrounding scope. The environment contains the captured values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Closure {
    /// The content-addressed hash of the function (lambda body).
    pub function_hash: blake3::Hash,

    /// The captured environment: values of free variables at closure creation time.
    /// The order matches the capture order during compilation.
    pub environment: Vec<Value>,
}

impl Closure {
    /// Create a new closure.
    #[must_use]
    pub fn new(function_hash: blake3::Hash, environment: Vec<Value>) -> Self {
        Self {
            function_hash,
            environment,
        }
    }
}

/// A first-class handler value that can handle an ability.
///
/// Handler values are created using handler literal syntax:
/// ```ambient
/// let mock_fs: Handler<Filesystem> = {
///   read(path) => resume("mock content"),
///   write(path, content) => resume(()),
///   exists(path) => resume(true),
/// };
/// ```
///
/// They can be composed with other handlers and used in handle expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerValue {
    /// The ability that this handler handles.
    pub ability_id: u16,

    /// Method implementations: `method_id` -> function hash that implements the handler.
    /// Each handler function receives implicit parameters: (continuation, `suspended_ability`)
    /// and can extract ability arguments from the suspended ability.
    pub methods: HashMap<u16, blake3::Hash>,

    /// Optional captured environment for closures within the handler.
    /// If handler methods capture variables from their surrounding scope,
    /// those values are stored here.
    pub captures: Vec<Value>,
}

impl HandlerValue {
    /// Create a new handler value.
    #[must_use]
    pub fn new(ability_id: u16, methods: HashMap<u16, blake3::Hash>) -> Self {
        Self {
            ability_id,
            methods,
            captures: Vec::new(),
        }
    }

    /// Create a new handler value with captured environment.
    #[must_use]
    pub fn with_captures(
        ability_id: u16,
        methods: HashMap<u16, blake3::Hash>,
        captures: Vec<Value>,
    ) -> Self {
        Self {
            ability_id,
            methods,
            captures,
        }
    }

    /// Get the handler function for a specific method.
    #[must_use]
    pub fn get_method(&self, method_id: u16) -> Option<blake3::Hash> {
        self.methods.get(&method_id).copied()
    }

    /// Check if this handler handles a specific method.
    #[must_use]
    pub fn handles_method(&self, method_id: u16) -> bool {
        self.methods.contains_key(&method_id)
    }

    /// Compose this handler with another, with `other` taking precedence.
    /// Both handlers must handle the same ability.
    #[must_use]
    pub fn compose(&self, other: &Self) -> Option<Self> {
        if self.ability_id != other.ability_id {
            return None;
        }

        let mut methods = self.methods.clone();
        methods.extend(other.methods.iter().map(|(k, v)| (*k, *v)));

        // Combine captures from both handlers
        let mut captures = self.captures.clone();
        captures.extend(other.captures.iter().cloned());

        Some(Self {
            ability_id: self.ability_id,
            methods,
            captures,
        })
    }
}

/// A captured continuation representing suspended computation.
///
/// Single-shot: can only be resumed once. Attempting to resume twice
/// is a runtime error.
#[derive(Debug)]
pub struct Continuation {
    /// The captured value stack segment.
    pub stack: Vec<Value>,

    /// The captured call frames.
    pub frames: Vec<CapturedFrame>,

    /// Whether this continuation has been resumed (single-shot enforcement).
    resumed: AtomicBool,
}

/// A captured call frame for continuations.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// The function hash being executed.
    pub function_hash: blake3::Hash,

    /// The instruction pointer when captured.
    pub ip: usize,

    /// The base pointer when captured.
    pub bp: usize,
}

impl Continuation {
    /// Create a new continuation.
    #[must_use]
    pub fn new(stack: Vec<Value>, frames: Vec<CapturedFrame>) -> Self {
        Self {
            stack,
            frames,
            resumed: AtomicBool::new(false),
        }
    }

    /// Check if this continuation has already been resumed.
    #[must_use]
    pub fn is_resumed(&self) -> bool {
        self.resumed.load(Ordering::Acquire)
    }

    /// Mark this continuation as resumed. Returns false if already resumed.
    ///
    /// Uses compare-and-swap to atomically check and set, ensuring thread safety.
    pub fn mark_resumed(&self) -> bool {
        self.resumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

impl Value {
    /// Create a new string value.
    #[must_use]
    pub fn string(s: impl Into<String>) -> Self {
        Self::String(Arc::new(s.into()))
    }

    /// Create a new tuple value.
    #[must_use]
    pub fn tuple(values: Vec<Value>) -> Self {
        Self::Tuple(Arc::new(values))
    }

    /// Create a new record value.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Arc<str>>, Value)>) -> Self {
        Self::Record(Arc::new(
            fields.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        ))
    }

    /// Returns the type name for error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Bool(_) => "bool",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Tuple(_) => "tuple",
            Self::Record(_) => "record",
            Self::FunctionRef(_) => "function",
            Self::SuspendedAbility(_) => "suspended_ability",
            Self::Continuation(_) => "continuation",
            Self::Closure(_) => "closure",
            Self::Handler(_) => "handler",
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Set(_) => "set",
        }
    }

    /// Create a new list value.
    #[must_use]
    pub fn list(values: Vec<Value>) -> Self {
        Self::List(Arc::new(values))
    }

    /// Create a new suspended ability value.
    #[must_use]
    pub fn suspended_ability(ability_id: u16, method_id: u16, args: Vec<Value>) -> Self {
        Self::SuspendedAbility(Arc::new(SuspendedAbility::new(ability_id, method_id, args)))
    }

    /// Create a new continuation value.
    #[must_use]
    pub fn continuation(stack: Vec<Value>, frames: Vec<CapturedFrame>) -> Self {
        Self::Continuation(Arc::new(Continuation::new(stack, frames)))
    }

    /// Create a new closure value.
    #[must_use]
    pub fn closure(function_hash: blake3::Hash, environment: Vec<Value>) -> Self {
        Self::Closure(Arc::new(Closure::new(function_hash, environment)))
    }

    /// Create a new handler value.
    #[must_use]
    pub fn handler(ability_id: u16, methods: HashMap<u16, blake3::Hash>) -> Self {
        Self::Handler(Arc::new(HandlerValue::new(ability_id, methods)))
    }

    /// Create a new handler value with captured environment.
    #[must_use]
    pub fn handler_with_captures(
        ability_id: u16,
        methods: HashMap<u16, blake3::Hash>,
        captures: Vec<Value>,
    ) -> Self {
        Self::Handler(Arc::new(HandlerValue::with_captures(
            ability_id, methods, captures,
        )))
    }

    /// Create a new empty map value.
    #[must_use]
    pub fn empty_map() -> Self {
        Self::Map(Arc::new(MapValue::new()))
    }

    /// Create a new map value from key-value pairs.
    #[must_use]
    pub fn map(entries: impl IntoIterator<Item = (impl Into<Arc<str>>, Value)>) -> Self {
        Self::Map(Arc::new(MapValue::from_entries(entries)))
    }

    /// Create a new empty set value.
    #[must_use]
    pub fn empty_set() -> Self {
        Self::Set(Arc::new(SetValue::new()))
    }

    /// Create a new set value from values.
    #[must_use]
    pub fn set(values: impl IntoIterator<Item = Value>) -> Self {
        Self::Set(Arc::new(SetValue::from_values(values)))
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // Type accessors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Extract the number if this value is a `Number`, otherwise `None`.
    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Extract the boolean if this value is a `Bool`, otherwise `None`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Extract the string if this value is a `String`, consuming self.
    #[must_use]
    pub fn into_string(self) -> Option<Arc<String>> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            // NaN != NaN per IEEE 754, but we want structural equality for values
            (Self::Number(a), Self::Number(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            // Tuples and lists are structurally equal
            (Self::Tuple(a), Self::Tuple(b)) | (Self::List(a), Self::List(b)) => a == b,
            (Self::Record(a), Self::Record(b)) => a == b,
            // Maps are structurally equal
            (Self::Map(a), Self::Map(b)) => a == b,
            // Sets are structurally equal
            (Self::Set(a), Self::Set(b)) => a == b,
            (Self::FunctionRef(a), Self::FunctionRef(b)) => a == b,
            // Suspended abilities are equal if they have the same ability/method/args
            (Self::SuspendedAbility(a), Self::SuspendedAbility(b)) => {
                a.ability_id == b.ability_id && a.method_id == b.method_id && a.args == b.args
            }
            // Continuations are identity-compared (same Arc)
            (Self::Continuation(a), Self::Continuation(b)) => Arc::ptr_eq(a, b),
            // Closures are equal if they have the same function and environment
            (Self::Closure(a), Self::Closure(b)) => {
                a.function_hash == b.function_hash && a.environment == b.environment
            }
            // Handlers are equal if they have the same ability, methods, and captures
            (Self::Handler(a), Self::Handler(b)) => {
                a.ability_id == b.ability_id && a.methods == b.methods && a.captures == b.captures
            }
            _ => false,
        }
    }
}

impl Eq for Value {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_equality() {
        assert_eq!(Value::Unit, Value::Unit);
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_eq!(Value::Number(42.0), Value::Number(42.0));
        assert_eq!(Value::string("hello"), Value::string("hello"));
        assert_eq!(
            Value::tuple(vec![Value::Number(1.0), Value::Bool(true)]),
            Value::tuple(vec![Value::Number(1.0), Value::Bool(true)])
        );
    }

    #[test]
    fn test_value_inequality() {
        assert_ne!(Value::Bool(true), Value::Bool(false));
        assert_ne!(Value::Number(1.0), Value::Number(2.0));
        assert_ne!(Value::string("a"), Value::string("b"));
        assert_ne!(Value::Unit, Value::Bool(false));
    }

    #[test]
    fn test_nan_equality() {
        // NaN should equal itself for structural comparison
        assert_eq!(Value::Number(f64::NAN), Value::Number(f64::NAN));
    }

    #[test]
    fn test_record_creation() {
        let record = Value::record([("x", Value::Number(1.0)), ("y", Value::Number(2.0))]);
        assert_eq!(record.type_name(), "record");
    }

    // =========================================================================
    // Milestone 3: Serialization Tests
    // =========================================================================

    #[test]
    fn test_serialize_primitives() {
        // Test serialization of primitive types
        let unit = Value::Unit;
        let bool_val = Value::Bool(true);
        let num_val = Value::Number(42.5);
        let str_val = Value::string("hello");

        // Round-trip through JSON
        let unit_json = serde_json::to_string(&unit).unwrap();
        let unit_back: Value = serde_json::from_str(&unit_json).unwrap();
        assert_eq!(unit, unit_back);

        let bool_json = serde_json::to_string(&bool_val).unwrap();
        let bool_back: Value = serde_json::from_str(&bool_json).unwrap();
        assert_eq!(bool_val, bool_back);

        let num_json = serde_json::to_string(&num_val).unwrap();
        let num_back: Value = serde_json::from_str(&num_json).unwrap();
        assert_eq!(num_val, num_back);

        let str_json = serde_json::to_string(&str_val).unwrap();
        let str_back: Value = serde_json::from_str(&str_json).unwrap();
        assert_eq!(str_val, str_back);
    }

    #[test]
    fn test_serialize_tuple() {
        let tuple = Value::tuple(vec![
            Value::Number(1.0),
            Value::Bool(false),
            Value::string("nested"),
        ]);

        let json = serde_json::to_string(&tuple).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(tuple, back);
    }

    #[test]
    fn test_serialize_record() {
        let record = Value::record([
            ("name", Value::string("Alice")),
            ("age", Value::Number(30.0)),
            ("active", Value::Bool(true)),
        ]);

        let json = serde_json::to_string(&record).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn test_serialize_function_ref() {
        let hash = blake3::hash(b"test::my_function");
        let func_ref = Value::FunctionRef(hash);

        let json = serde_json::to_string(&func_ref).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(func_ref, back);
    }

    #[test]
    fn test_serialize_suspended_ability() {
        // Create a suspended ability with arguments
        let ability = Value::suspended_ability(
            0x0001, // Console
            0x0000, // print
            vec![Value::string("Hello, world!")],
        );

        let json = serde_json::to_string(&ability).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(ability, back);
    }

    #[test]
    fn test_serialize_suspended_ability_multiple_args() {
        // Create a suspended ability with multiple arguments of different types
        let ability = Value::suspended_ability(
            0x0002, // Math
            0x0001, // add
            vec![
                Value::Number(10.0),
                Value::Number(32.0),
                Value::string("extra"),
                Value::tuple(vec![Value::Bool(true)]),
            ],
        );

        let json = serde_json::to_string(&ability).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(ability, back);
    }

    #[test]
    fn test_serialize_nested_structures() {
        // Create deeply nested structure containing ability values
        let inner_ability = Value::suspended_ability(0x0001, 0x0000, vec![Value::Number(42.0)]);

        let record = Value::record([
            ("operation", inner_ability.clone()),
            ("label", Value::string("test op")),
        ]);

        let tuple = Value::tuple(vec![record.clone(), inner_ability, Value::Number(123.0)]);

        let json = serde_json::to_string(&tuple).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(tuple, back);
    }

    #[test]
    fn test_serialize_ability_preserves_ids() {
        // Verify that ability_id and method_id are correctly preserved
        let ability = Value::suspended_ability(0x1234, 0x5678, vec![Value::Unit]);

        let json = serde_json::to_string(&ability).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();

        if let Value::SuspendedAbility(a) = back {
            assert_eq!(a.ability_id, 0x1234);
            assert_eq!(a.method_id, 0x5678);
        } else {
            panic!("Expected SuspendedAbility, got something else");
        }
    }
}
