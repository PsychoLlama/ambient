//! Runtime value types for the Ambient language.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ambient_core::AbilityId;
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

    /// Byte sequence for binary data.
    Bytes(Arc<Vec<u8>>),

    /// Tuple: fixed-size, heterogeneous collection accessed by index.
    Tuple(Arc<Vec<Value>>),

    /// Record: named fields with values, structural typing.
    Record(Arc<HashMap<Arc<str>, Value>>),

    /// Reference to a content-addressed function.
    FunctionRef(blake3::Hash),

    /// Reference to a content-addressed ability interface.
    ///
    /// Appears in compiled constant pools: `Suspend`/`Handle`/`MakeHandler`
    /// name the ability they target through one of these.
    AbilityRef(AbilityId),

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

    /// An enum variant instance.
    /// Contains the variant tag (index) and optional payload value.
    /// Used for types like `Option<T>` and `Result<T, E>`.
    Enum(Arc<EnumValue>),

    /// A module value for REPL introspection.
    /// Contains the module path and a list of its exports.
    Module(Arc<ModuleValue>),

    /// A reference to a module member (function, constant, etc.) for REPL introspection.
    /// Contains the full path and the kind of the member.
    ModuleMember(Arc<ModuleMemberRef>),
}

/// Reference to a module member for introspection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleMemberRef {
    /// Full path to the member (e.g., "core.list.first").
    pub path: Arc<str>,
    /// The kind of member.
    pub kind: ModuleExportKind,
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

/// An enum variant instance.
///
/// Represents a specific variant of an enum type at runtime.
/// The variant is identified by its tag (index) and may have a payload.
///
/// ## Well-known enum types
///
/// The standard library provides these built-in enum types:
///
/// - `Option<T>`: `None` (tag 0) or `Some(T)` (tag 1)
/// - `Result<T, E>`: `Ok(T)` (tag 0) or `Err(E)` (tag 1)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnumValue {
    /// The name of the enum type (e.g., "Option", "Result").
    /// Used for type checking and display.
    pub type_name: Arc<str>,

    /// The variant tag (index into the enum's variant list).
    /// For Option: 0 = None, 1 = Some
    /// For Result: 0 = Ok, 1 = Err
    pub tag: u16,

    /// The name of the variant (e.g., "Some", "None", "Ok", "Err").
    pub variant_name: Arc<str>,

    /// Optional payload value. `None` for unit variants (like `Option::None`).
    pub payload: Option<Box<Value>>,
}

impl EnumValue {
    /// Create a new enum variant.
    #[must_use]
    pub fn new(
        type_name: impl Into<Arc<str>>,
        tag: u16,
        variant_name: impl Into<Arc<str>>,
        payload: Option<Value>,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            tag,
            variant_name: variant_name.into(),
            payload: payload.map(Box::new),
        }
    }

    /// Create an `Option::None` value.
    #[must_use]
    pub fn none() -> Self {
        Self::new("Option", 0, "None", None)
    }

    /// Create an `Option::Some(value)` value.
    #[must_use]
    pub fn some(value: Value) -> Self {
        Self::new("Option", 1, "Some", Some(value))
    }

    /// Create a `Result::Ok(value)` value.
    #[must_use]
    pub fn ok(value: Value) -> Self {
        Self::new("Result", 0, "Ok", Some(value))
    }

    /// Create a `Result::Err(error)` value.
    #[must_use]
    pub fn err(error: Value) -> Self {
        Self::new("Result", 1, "Err", Some(error))
    }

    /// Check if this is a specific variant by tag.
    #[must_use]
    pub fn is_variant(&self, tag: u16) -> bool {
        self.tag == tag
    }

    /// Check if this is a specific variant by name.
    #[must_use]
    pub fn is_variant_named(&self, name: &str) -> bool {
        &*self.variant_name == name
    }

    /// Get the payload, if any.
    #[must_use]
    pub fn payload(&self) -> Option<&Value> {
        self.payload.as_deref()
    }

    /// Take the payload, consuming self.
    #[must_use]
    pub fn into_payload(self) -> Option<Value> {
        self.payload.map(|b| *b)
    }
}

/// A module value for REPL introspection.
///
/// Represents a module that can be displayed in the REPL, showing its
/// path and list of exports (functions, constants, types, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleValue {
    /// The module path (e.g., "pkg.utils" or "core.math").
    pub path: Arc<str>,

    /// Exported symbols from this module.
    pub exports: Vec<ModuleExport>,
}

/// An exported symbol from a module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleExport {
    /// The name of the exported symbol.
    pub name: Arc<str>,

    /// The kind of export (function, constant, type, etc.).
    pub kind: ModuleExportKind,

    /// Optional type signature for display (e.g., "(xs: List<a>, f: (a) -> b): List<b>").
    pub signature: Option<Arc<str>>,
}

/// The kind of exported symbol in a module.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModuleExportKind {
    /// A function.
    Function,
    /// A constant.
    Const,
    /// A type alias.
    Type,
    /// An enum type.
    Enum,
    /// An enum variant.
    Variant,
    /// An ability.
    Ability,
    /// A submodule.
    Module,
}

impl ModuleValue {
    /// Create a new module value.
    #[must_use]
    pub fn new(path: impl Into<Arc<str>>, exports: Vec<ModuleExport>) -> Self {
        Self {
            path: path.into(),
            exports,
        }
    }
}

impl ModuleExport {
    /// Create a new module export without a signature.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, kind: ModuleExportKind) -> Self {
        Self {
            name: name.into(),
            kind,
            signature: None,
        }
    }

    /// Create a new module export with a signature.
    #[must_use]
    pub fn with_signature(
        name: impl Into<Arc<str>>,
        kind: ModuleExportKind,
        signature: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            signature: Some(signature.into()),
        }
    }
}

/// A suspended ability operation waiting to be performed.
///
/// This type is fully serializable, allowing ability values to be stored,
/// transmitted, and executed remotely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendedAbility {
    /// The content-addressed identity of the ability being invoked.
    pub ability_id: AbilityId,

    /// The method being called on the ability (e.g., "read", "print").
    pub method_id: u16,

    /// The arguments to pass to the ability method.
    pub args: Vec<Value>,
}

impl SuspendedAbility {
    /// Create a new suspended ability.
    #[must_use]
    pub fn new(ability_id: AbilityId, method_id: u16, args: Vec<Value>) -> Self {
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
    /// The content-addressed identity of the ability this handler handles.
    pub ability_id: AbilityId,

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
    pub fn new(ability_id: AbilityId, methods: HashMap<u16, blake3::Hash>) -> Self {
        Self {
            ability_id,
            methods,
            captures: Vec::new(),
        }
    }

    /// Create a new handler value with captured environment.
    #[must_use]
    pub fn with_captures(
        ability_id: AbilityId,
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
/// Everything inside is *relative to the continuation's base*: the base
/// is the stack height and frame index of the delimiting handler boundary
/// at capture time. On resume, the VM rebases all offsets onto the current
/// stack and frame heights, so a continuation can be resumed from a
/// different context than it was captured in.
///
/// Single-shot: can only be resumed once. Attempting to resume twice
/// is a runtime error.
#[derive(Debug)]
pub struct Continuation {
    /// The captured value stack segment (from the boundary frame's base
    /// pointer upward).
    pub stack: Vec<Value>,

    /// The captured call frames, base pointers relative to `stack`.
    pub frames: Vec<CapturedFrame>,

    /// Handler entries that delimited regions within the captured
    /// computation, boundary indexes relative to `frames`. Re-installed
    /// on resume so performs in the resumed body dispatch exactly as
    /// they would have originally (deep handler semantics).
    pub handlers: Vec<CapturedHandler>,

    /// Whether this continuation has been resumed (single-shot enforcement).
    resumed: AtomicBool,
}

/// Action to perform when a call frame returns.
///
/// This enables "continuation frames" for operations like `Option.map`
/// that call a closure and then wrap the result in an enum.
#[derive(Debug, Clone, Default)]
pub enum ReturnAction {
    /// Normal return - just push the result onto the caller's stack.
    #[default]
    None,

    /// Wrap the result in `Some(result)` for `Option.map`.
    WrapSome,

    /// For `Option.and_then` - the closure returns `Option<U>`, pass through as-is.
    /// This is essentially the same as `None` but documents intent.
    PassThrough,

    /// Wrap the result in `Ok(result)` for `Result.map`.
    WrapOk,

    /// Wrap the result in `Err(result)` for `Result.map_err`.
    WrapErr,
}

/// A captured call frame for continuations.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// The function hash being executed.
    pub function_hash: blake3::Hash,

    /// The instruction pointer when captured.
    pub ip: usize,

    /// The base pointer, relative to the continuation's captured stack.
    pub bp: usize,

    /// Captured closure environment of the frame (empty for plain calls).
    pub captures: Vec<Value>,

    /// The frame's pending return action (e.g. enum wrapping for
    /// intrinsic closure calls), preserved across suspension.
    pub return_action: ReturnAction,
}

/// The implementation behind an installed ability handler.
#[derive(Debug, Clone)]
pub enum HandlerImpl {
    /// An inline handler arm: one function for all methods of the ability,
    /// with the environment it captured from its enclosing scope.
    Inline {
        /// The compiled arm function (receives continuation + suspended ability).
        func: blake3::Hash,
        /// Values captured from the scope enclosing the handle expression.
        captures: Vec<Value>,
    },

    /// A first-class handler value with per-method functions.
    Value {
        /// The handler value (methods + captures).
        handler: Arc<HandlerValue>,
    },
}

/// A handler entry captured into a continuation.
#[derive(Debug, Clone)]
pub struct CapturedHandler {
    /// The ability this handler intercepts.
    pub ability_id: AbilityId,

    /// The handler implementation.
    pub handler: HandlerImpl,

    /// The delimiting frame index, relative to the continuation's frames.
    pub boundary: usize,
}

impl Continuation {
    /// Create a new continuation.
    #[must_use]
    pub fn new(
        stack: Vec<Value>,
        frames: Vec<CapturedFrame>,
        handlers: Vec<CapturedHandler>,
    ) -> Self {
        Self {
            stack,
            frames,
            handlers,
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

    /// Create a new bytes value.
    #[must_use]
    pub fn bytes(data: Vec<u8>) -> Self {
        Self::Bytes(Arc::new(data))
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
            Self::Bytes(_) => "bytes",
            Self::Tuple(_) => "tuple",
            Self::Record(_) => "record",
            Self::FunctionRef(_) => "function",
            Self::AbilityRef(_) => "ability",
            Self::SuspendedAbility(_) => "suspended_ability",
            Self::Continuation(_) => "continuation",
            Self::Closure(_) => "closure",
            Self::Handler(_) => "handler",
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Set(_) => "set",
            Self::Enum(_) => "enum",
            Self::Module(_) => "module",
            Self::ModuleMember(_) => "module_member",
        }
    }

    /// Create a new list value.
    #[must_use]
    pub fn list(values: Vec<Value>) -> Self {
        Self::List(Arc::new(values))
    }

    /// Create a new suspended ability value.
    #[must_use]
    pub fn suspended_ability(ability_id: AbilityId, method_id: u16, args: Vec<Value>) -> Self {
        Self::SuspendedAbility(Arc::new(SuspendedAbility::new(ability_id, method_id, args)))
    }

    /// Create a new continuation value.
    #[must_use]
    pub fn continuation(
        stack: Vec<Value>,
        frames: Vec<CapturedFrame>,
        handlers: Vec<CapturedHandler>,
    ) -> Self {
        Self::Continuation(Arc::new(Continuation::new(stack, frames, handlers)))
    }

    /// Create a new closure value.
    #[must_use]
    pub fn closure(function_hash: blake3::Hash, environment: Vec<Value>) -> Self {
        Self::Closure(Arc::new(Closure::new(function_hash, environment)))
    }

    /// Create a new handler value.
    #[must_use]
    pub fn handler(ability_id: AbilityId, methods: HashMap<u16, blake3::Hash>) -> Self {
        Self::Handler(Arc::new(HandlerValue::new(ability_id, methods)))
    }

    /// Create a new handler value with captured environment.
    #[must_use]
    pub fn handler_with_captures(
        ability_id: AbilityId,
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

    /// Create a new enum variant value.
    #[must_use]
    pub fn enum_variant(
        type_name: impl Into<Arc<str>>,
        tag: u16,
        variant_name: impl Into<Arc<str>>,
        payload: Option<Value>,
    ) -> Self {
        Self::Enum(Arc::new(EnumValue::new(
            type_name,
            tag,
            variant_name,
            payload,
        )))
    }

    /// Create an `Option::None` value.
    #[must_use]
    pub fn none() -> Self {
        Self::Enum(Arc::new(EnumValue::none()))
    }

    /// Create an `Option::Some(value)` value.
    #[must_use]
    pub fn some(value: Value) -> Self {
        Self::Enum(Arc::new(EnumValue::some(value)))
    }

    /// Create a `Result::Ok(value)` value.
    #[must_use]
    pub fn ok(value: Value) -> Self {
        Self::Enum(Arc::new(EnumValue::ok(value)))
    }

    /// Create a `Result::Err(error)` value.
    #[must_use]
    pub fn err(error: Value) -> Self {
        Self::Enum(Arc::new(EnumValue::err(error)))
    }

    /// Create a module value.
    #[must_use]
    pub fn module(path: impl Into<Arc<str>>, exports: Vec<ModuleExport>) -> Self {
        Self::Module(Arc::new(ModuleValue::new(path, exports)))
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

    /// Extract the bytes if this value is a `Bytes`, otherwise `None`.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b.as_slice()),
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
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            // Tuples and lists are structurally equal
            (Self::Tuple(a), Self::Tuple(b)) | (Self::List(a), Self::List(b)) => a == b,
            (Self::Record(a), Self::Record(b)) => a == b,
            // Maps are structurally equal
            (Self::Map(a), Self::Map(b)) => a == b,
            // Sets are structurally equal
            (Self::Set(a), Self::Set(b)) => a == b,
            (Self::FunctionRef(a), Self::FunctionRef(b)) => a == b,
            (Self::AbilityRef(a), Self::AbilityRef(b)) => a == b,
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
            // Enum values are structurally equal
            (Self::Enum(a), Self::Enum(b)) => a == b,
            // Modules are structurally equal
            (Self::Module(a), Self::Module(b)) => a == b,
            // Module members are structurally equal
            (Self::ModuleMember(a), Self::ModuleMember(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_unit() {
        let v = Value::Unit;
        assert_eq!(v.type_name(), "unit");
        assert_eq!(v, Value::Unit);
    }

    #[test]
    fn test_value_bool() {
        assert_eq!(Value::Bool(true).type_name(), "bool");
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Bool(false).as_bool(), Some(false));
        assert!(Value::Bool(true) != Value::Bool(false));
    }

    #[test]
    fn test_value_number() {
        assert_eq!(Value::Number(42.0).type_name(), "number");
        assert_eq!(Value::Number(42.0).as_number(), Some(42.0));
        assert_eq!(Value::Number(1.5), Value::Number(1.5));
    }

    #[test]
    fn test_value_number_nan_equality() {
        // NaN should equal itself for structural equality (uses to_bits)
        let nan = Value::Number(f64::NAN);
        assert_eq!(nan, nan);
    }

    #[test]
    fn test_value_string() {
        let v = Value::string("hello");
        assert_eq!(v.type_name(), "string");
        if let Value::String(s) = &v {
            assert_eq!(&**s, "hello");
        } else {
            panic!("Expected string");
        }
    }

    #[test]
    fn test_value_tuple() {
        let v = Value::tuple(vec![Value::Number(1.0), Value::Bool(true)]);
        assert_eq!(v.type_name(), "tuple");
        if let Value::Tuple(t) = v {
            assert_eq!(t.len(), 2);
        }
    }

    #[test]
    fn test_value_record() {
        let v = Value::record([("x", Value::Number(10.0)), ("y", Value::Number(20.0))]);
        assert_eq!(v.type_name(), "record");
        if let Value::Record(r) = v {
            assert_eq!(r.get(&Arc::from("x")), Some(&Value::Number(10.0)));
        }
    }

    #[test]
    fn test_value_list() {
        let v = Value::list(vec![Value::Number(1.0), Value::Number(2.0)]);
        assert_eq!(v.type_name(), "list");
        if let Value::List(l) = v {
            assert_eq!(l.len(), 2);
        }
    }

    #[test]
    fn test_value_option_none() {
        let v = Value::none();
        assert_eq!(v.type_name(), "enum");
        if let Value::Enum(e) = &v {
            assert_eq!(&*e.type_name, "Option");
            assert_eq!(e.tag, 0);
            assert!(e.payload.is_none());
        }
    }

    #[test]
    fn test_value_option_some() {
        let v = Value::some(Value::Number(42.0));
        if let Value::Enum(e) = &v {
            assert_eq!(&*e.type_name, "Option");
            assert_eq!(e.tag, 1);
            assert_eq!(e.payload(), Some(&Value::Number(42.0)));
        }
    }

    #[test]
    fn test_value_result_ok() {
        let v = Value::ok(Value::string("success"));
        if let Value::Enum(e) = &v {
            assert_eq!(&*e.type_name, "Result");
            assert_eq!(e.tag, 0);
            assert!(e.payload.is_some());
        }
    }

    #[test]
    fn test_value_result_err() {
        let v = Value::err(Value::string("error"));
        if let Value::Enum(e) = &v {
            assert_eq!(&*e.type_name, "Result");
            assert_eq!(e.tag, 1);
        }
    }

    #[test]
    fn test_map_value_operations() {
        let map = MapValue::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        let map = map.insert("key1", Value::Number(1.0));
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("key1"));
        assert_eq!(map.get("key1"), Some(&Value::Number(1.0)));

        let map = map.insert("key2", Value::Number(2.0));
        assert_eq!(map.len(), 2);
        assert_eq!(map.keys().len(), 2);
        assert_eq!(map.values().len(), 2);

        let map = map.remove("key1");
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("key1"));
    }

    #[test]
    fn test_set_value_operations() {
        let set = SetValue::new();
        assert!(set.is_empty());

        let set = set.insert(Value::Number(1.0));
        assert!(!set.is_empty());
        assert_eq!(set.len(), 1);
        assert!(set.contains(&Value::Number(1.0)));

        // Insert duplicate should not change size
        let set = set.insert(Value::Number(1.0));
        assert_eq!(set.len(), 1);

        let set = set.insert(Value::Number(2.0));
        assert_eq!(set.len(), 2);

        let set = set.remove(&Value::Number(1.0));
        assert_eq!(set.len(), 1);
        assert!(!set.contains(&Value::Number(1.0)));
    }

    #[test]
    fn test_set_value_union() {
        let set1 = SetValue::from_values([Value::Number(1.0), Value::Number(2.0)]);
        let set2 = SetValue::from_values([Value::Number(2.0), Value::Number(3.0)]);
        let union = set1.union(&set2);
        assert_eq!(union.len(), 3);
    }

    #[test]
    fn test_set_value_intersection() {
        let set1 = SetValue::from_values([Value::Number(1.0), Value::Number(2.0)]);
        let set2 = SetValue::from_values([Value::Number(2.0), Value::Number(3.0)]);
        let intersection = set1.intersection(&set2);
        assert_eq!(intersection.len(), 1);
        assert!(intersection.contains(&Value::Number(2.0)));
    }

    #[test]
    fn test_set_value_difference() {
        let set1 = SetValue::from_values([Value::Number(1.0), Value::Number(2.0)]);
        let set2 = SetValue::from_values([Value::Number(2.0), Value::Number(3.0)]);
        let diff = set1.difference(&set2);
        assert_eq!(diff.len(), 1);
        assert!(diff.contains(&Value::Number(1.0)));
    }

    #[test]
    fn test_enum_value_is_variant() {
        let some = EnumValue::some(Value::Unit);
        assert!(some.is_variant(1));
        assert!(!some.is_variant(0));
        assert!(some.is_variant_named("Some"));
        assert!(!some.is_variant_named("None"));
    }

    #[test]
    fn test_enum_value_into_payload() {
        let some = EnumValue::some(Value::Number(42.0));
        let payload = some.into_payload();
        assert_eq!(payload, Some(Value::Number(42.0)));

        let none = EnumValue::none();
        assert_eq!(none.into_payload(), None);
    }

    #[test]
    fn test_suspended_ability() {
        let id = AbilityId::from_bytes([7; 32]);
        let ability = SuspendedAbility::new(id, 2, vec![Value::Number(42.0)]);
        assert_eq!(ability.ability_id, id);
        assert_eq!(ability.method_id, 2);
        assert_eq!(ability.args.len(), 1);
    }

    #[test]
    fn test_continuation_single_shot() {
        let cont = Continuation::new(vec![], vec![], vec![]);
        assert!(!cont.is_resumed());
        assert!(cont.mark_resumed());
        assert!(cont.is_resumed());
        // Second resume should fail
        assert!(!cont.mark_resumed());
    }

    #[test]
    fn test_handler_value_methods() {
        let mut methods = HashMap::new();
        methods.insert(0u16, blake3::hash(b"test"));
        let id = AbilityId::from_bytes([7; 32]);
        let handler = HandlerValue::new(id, methods);

        assert_eq!(handler.ability_id, id);
        assert!(handler.handles_method(0));
        assert!(!handler.handles_method(1));
        assert!(handler.get_method(0).is_some());
        assert!(handler.get_method(1).is_none());
    }

    #[test]
    fn test_handler_value_compose() {
        let mut methods1 = HashMap::new();
        methods1.insert(0u16, blake3::hash(b"method0"));
        let handler1 = HandlerValue::new(AbilityId::from_bytes([7; 32]), methods1);

        let mut methods2 = HashMap::new();
        methods2.insert(1u16, blake3::hash(b"method1"));
        let handler2 = HandlerValue::new(AbilityId::from_bytes([7; 32]), methods2);

        let composed = handler1.compose(&handler2);
        assert!(composed.is_some());
        let composed = composed.expect("compose should succeed");
        assert!(composed.handles_method(0));
        assert!(composed.handles_method(1));
    }

    #[test]
    fn test_handler_value_compose_different_abilities_fails() {
        let handler1 = HandlerValue::new(AbilityId::from_bytes([1; 32]), HashMap::new());
        let handler2 = HandlerValue::new(AbilityId::from_bytes([2; 32]), HashMap::new());
        assert!(handler1.compose(&handler2).is_none());
    }

    #[test]
    fn test_module_export() {
        let export = ModuleExport::new("my_func", ModuleExportKind::Function);
        assert_eq!(&*export.name, "my_func");
        assert_eq!(export.kind, ModuleExportKind::Function);
        assert!(export.signature.is_none());

        let export_with_sig = ModuleExport::with_signature(
            "add",
            ModuleExportKind::Function,
            "(a: number, b: number): number",
        );
        assert!(export_with_sig.signature.is_some());
    }

    #[test]
    fn test_module_value() {
        let exports = vec![
            ModuleExport::new("func1", ModuleExportKind::Function),
            ModuleExport::new("const1", ModuleExportKind::Const),
        ];
        let module = ModuleValue::new("pkg.utils", exports);
        assert_eq!(&*module.path, "pkg.utils");
        assert_eq!(module.exports.len(), 2);
    }

    #[test]
    fn test_value_equality() {
        // Tuples
        let t1 = Value::tuple(vec![Value::Number(1.0)]);
        let t2 = Value::tuple(vec![Value::Number(1.0)]);
        assert_eq!(t1, t2);

        // Records
        let r1 = Value::record([("a", Value::Number(1.0))]);
        let r2 = Value::record([("a", Value::Number(1.0))]);
        assert_eq!(r1, r2);

        // Lists
        let l1 = Value::list(vec![Value::Bool(true)]);
        let l2 = Value::list(vec![Value::Bool(true)]);
        assert_eq!(l1, l2);
    }
}
