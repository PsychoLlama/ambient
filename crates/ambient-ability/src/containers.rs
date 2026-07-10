//! Container values: `Map<K, V>` and `Set<T>`.
//!
//! Both compare elements/keys by `Value::eq` and preserve insertion order
//! for deterministic serialization.

use serde::{Deserialize, Serialize};

use crate::value::Value;

/// A map value keyed by arbitrary values: keys compare by `Value::eq` (like
/// `Set<T>`), so numbers, tuples, and records all work. Entries are a `Vec` in
/// insertion order; `insert` replaces a key in place. There is no `Ord` here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MapValue {
    /// The key-value pairs, in insertion order. Keys are unique by `Value::eq`.
    pub entries: Vec<(Value, Value)>,
}

impl MapValue {
    /// Create a new empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a map from key-value pairs; a later equal key replaces in place.
    pub fn from_entries(iter: impl IntoIterator<Item = (Value, Value)>) -> Self {
        iter.into_iter()
            .fold(Self::new(), |map, (k, v)| map.insert(k, v))
    }

    /// Get a value by key.
    #[must_use]
    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Insert a key-value pair, returning a new map. An existing key keeps its
    /// position and has only its value replaced.
    #[must_use]
    pub fn insert(&self, key: Value, value: Value) -> Self {
        let mut entries = self.entries.clone();
        if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            entries.push((key, value));
        }
        Self { entries }
    }

    /// Remove a key, returning a new map.
    #[must_use]
    pub fn remove(&self, key: &Value) -> Self {
        let entries = self.entries.iter().filter(|(k, _)| k != key);
        Self {
            entries: entries.cloned().collect(),
        }
    }

    /// Check if the map contains a key.
    #[must_use]
    pub fn contains_key(&self, key: &Value) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
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

    /// Get all keys as a list, in insertion order.
    #[must_use]
    pub fn keys(&self) -> Vec<Value> {
        self.entries.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Get all values as a list, in insertion order.
    #[must_use]
    pub fn values(&self) -> Vec<Value> {
        self.entries.iter().map(|(_, v)| v.clone()).collect()
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
