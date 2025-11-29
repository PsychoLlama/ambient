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
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Record(a), Self::Record(b)) => a == b,
            (Self::FunctionRef(a), Self::FunctionRef(b)) => a == b,
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
