use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

/// Represents a runtime value in the language.
///
/// Values are immutable and use reference counting for efficient sharing of
/// heap-allocated data (strings, tuples, records).
#[derive(Debug, Clone)]
pub enum Value {
    /// Unit type, represents absence of a meaningful value.
    Unit,

    /// Boolean value.
    Bool(bool),

    /// 64-bit floating point number (the only numeric type per spec).
    Number(f64),

    /// UTF-8 string.
    String(Rc<String>),

    /// Tuple: fixed-size, heterogeneous collection accessed by index.
    Tuple(Rc<Vec<Value>>),

    /// Record: named fields with values, structural typing.
    Record(Rc<HashMap<Rc<str>, Value>>),

    /// Reference to a content-addressed function.
    FunctionRef(blake3::Hash),

    /// A suspended ability operation that can be performed later.
    ///
    /// Contains the ability ID, method ID, and arguments.
    SuspendedAbility(Rc<SuspendedAbility>),

    /// A captured continuation that can be resumed (single-shot).
    Continuation(Rc<Continuation>),
}

/// A suspended ability operation waiting to be performed.
#[derive(Debug, Clone)]
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
    pub resumed: Cell<bool>,
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
            resumed: Cell::new(false),
        }
    }

    /// Check if this continuation has already been resumed.
    #[must_use]
    pub fn is_resumed(&self) -> bool {
        self.resumed.get()
    }

    /// Mark this continuation as resumed. Returns false if already resumed.
    pub fn mark_resumed(&self) -> bool {
        if self.resumed.get() {
            false
        } else {
            self.resumed.set(true);
            true
        }
    }
}

impl Value {
    /// Create a new string value.
    #[must_use]
    pub fn string(s: impl Into<String>) -> Self {
        Self::String(Rc::new(s.into()))
    }

    /// Create a new tuple value.
    #[must_use]
    pub fn tuple(values: Vec<Value>) -> Self {
        Self::Tuple(Rc::new(values))
    }

    /// Create a new record value.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Rc<str>>, Value)>) -> Self {
        Self::Record(Rc::new(fields.into_iter().map(|(k, v)| (k.into(), v)).collect()))
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
        }
    }

    /// Create a new suspended ability value.
    #[must_use]
    pub fn suspended_ability(ability_id: u16, method_id: u16, args: Vec<Value>) -> Self {
        Self::SuspendedAbility(Rc::new(SuspendedAbility::new(ability_id, method_id, args)))
    }

    /// Create a new continuation value.
    #[must_use]
    pub fn continuation(stack: Vec<Value>, frames: Vec<CapturedFrame>) -> Self {
        Self::Continuation(Rc::new(Continuation::new(stack, frames)))
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
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Record(a), Self::Record(b)) => a == b,
            (Self::FunctionRef(a), Self::FunctionRef(b)) => a == b,
            // Suspended abilities are equal if they have the same ability/method/args
            (Self::SuspendedAbility(a), Self::SuspendedAbility(b)) => {
                a.ability_id == b.ability_id
                    && a.method_id == b.method_id
                    && a.args == b.args
            }
            // Continuations are identity-compared (same Rc)
            (Self::Continuation(a), Self::Continuation(b)) => Rc::ptr_eq(a, b),
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
        let record = Value::record([
            ("x", Value::Number(1.0)),
            ("y", Value::Number(2.0)),
        ]);
        assert_eq!(record.type_name(), "record");
    }
}
