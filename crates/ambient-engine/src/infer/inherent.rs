//! Inherent (trait-less) impl registration and lookup.
//!
//! `impl Money { ... }` or `impl<T> Option<T> { ... }` attach methods
//! directly to a type. Dispatch is static, exactly like trait dispatch:
//! the checker resolves `value.method()` / `Type::method()` to a canonical
//! symbol, and the compiler links that symbol to a content hash. This
//! registry is a checker-side construct only — nothing survives to runtime.
//!
//! Coherence is enforced at the granularity of [`ImplKey`] plus method
//! name, which is exactly the granularity of the dispatch symbol: a
//! registration that finds no duplicate guarantees the symbol resolves
//! unambiguously everywhere in the compilation context. See the
//! architecture reference ("Inherent impls") for why this local rule is
//! sufficient under live upgrade.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use super::env::Scheme;
use crate::types::Type;

/// Identity of an impl target — the granularity of coherence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImplKey {
    /// A nominal type, identified by its UUID.
    Nominal(Uuid),
    /// A named type constructor identified by head name: built-in
    /// containers (`Option`, `Result`, `List`, `Map`, `Set`), declared
    /// enums, and the primitives under their type names (`number`,
    /// `string`, `bool`, `Bytes`).
    Named(Arc<str>),
}

/// A registered inherent method.
#[derive(Debug, Clone)]
pub struct InherentMethod {
    /// Method name.
    pub name: Arc<str>,
    /// Whether the method takes `self` (dot dispatch) or is associated
    /// (`Type::method(...)` call form).
    pub has_self: bool,
    /// Full function scheme — `(self, params...) -> ret with abilities`,
    /// quantified over the impl's and the method's type parameters. `self`
    /// is parameter 0 when `has_self` is true.
    pub scheme: Scheme,
    /// Canonical dispatch symbol (see [`inherent_method_symbol`]).
    pub symbol: Arc<str>,
}

/// The canonical function symbol for an inherent method.
///
/// Two segments (`<type-identity>::<method>`) so it can never collide with
/// trait impl symbols (`<uuid>::<Trait>::<method>`, three segments). Like
/// trait symbols, it is derived only from source-stable data, so it is
/// deterministic across compilation contexts. The type identity is the
/// same key coherence is enforced on, so "registration found no duplicate"
/// is exactly "this symbol resolves uniquely".
#[must_use]
pub fn inherent_method_symbol(key: &ImplKey, method: &str) -> Arc<str> {
    match key {
        ImplKey::Nominal(uuid) => format!("{uuid}::{method}").into(),
        ImplKey::Named(name) => format!("{name}::{method}").into(),
    }
}

/// Compute the impl-target key for a type, if the type can carry inherent
/// methods. Structural types (records, tuples, functions) have no stable
/// identity to attach methods to and return `None`.
#[must_use]
pub fn impl_key_for(ty: &Type) -> Option<ImplKey> {
    match ty {
        Type::Nominal(n) => Some(ImplKey::Nominal(n.uuid)),
        Type::Named(n) => Some(ImplKey::Named(Arc::clone(&n.name))),
        Type::Number => Some(ImplKey::Named("number".into())),
        Type::String => Some(ImplKey::Named("string".into())),
        Type::Bool => Some(ImplKey::Named("bool".into())),
        Type::Bytes => Some(ImplKey::Named("Bytes".into())),
        _ => None,
    }
}

/// Registry of inherent methods, keyed by impl target.
#[derive(Debug, Clone, Default)]
pub struct InherentRegistry {
    methods: HashMap<ImplKey, HashMap<Arc<str>, InherentMethod>>,
}

impl InherentRegistry {
    /// Register a method for a target type.
    ///
    /// Returns the previously registered method with the same name for the
    /// same target, if any — a coherence violation the caller must report:
    /// the two definitions would compete for one dispatch symbol.
    pub fn register(&mut self, key: ImplKey, method: InherentMethod) -> Option<InherentMethod> {
        self.methods
            .entry(key)
            .or_default()
            .insert(Arc::clone(&method.name), method)
    }

    /// Look up a method by target key and name.
    #[must_use]
    pub fn get(&self, key: &ImplKey, method: &str) -> Option<&InherentMethod> {
        self.methods.get(key)?.get(method)
    }
}
