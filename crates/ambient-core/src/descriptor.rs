//! Ability descriptors and provider trait.
//!
//! This module defines the interface for describing abilities in a way that
//! can be used by the type system and compiler without depending on engine types.

use crate::{AbilityId, MethodId};

/// Describes the type signature of an ability method.
///
/// This uses a factory function to defer type construction, allowing
/// the core crate to describe types without depending on the engine's
/// type system directly.
#[derive(Clone)]
pub struct MethodSignature<T> {
    /// Number of parameters the method takes.
    pub param_count: usize,

    /// Factory function that creates parameter types.
    /// Called with a type factory to construct engine-specific types.
    pub param_types: fn(&dyn TypeFactory<T>) -> Vec<T>,

    /// Factory function that creates the return type.
    pub return_type: fn(&dyn TypeFactory<T>) -> T,
}

/// Factory trait for creating engine-specific types.
///
/// The engine implements this trait to allow ability descriptors to
/// create types without depending on the engine directly.
pub trait TypeFactory<T> {
    /// Create the Unit type.
    fn unit(&self) -> T;

    /// Create the Bool type.
    fn bool(&self) -> T;

    /// Create the Number type.
    fn number(&self) -> T;

    /// Create the String type.
    fn string(&self) -> T;

    /// Create the Never type (for expressions that don't return).
    fn never(&self) -> T;

    /// Create a fresh type variable (for polymorphic methods).
    fn type_var(&self) -> T;

    /// Create a List<T> type.
    fn list(&self, element: T) -> T;
}

/// Describes a single method on an ability.
#[derive(Clone)]
pub struct MethodDescriptor<T> {
    /// Method ID (unique within the ability).
    pub id: MethodId,

    /// Method name as it appears in source code.
    pub name: &'static str,

    /// Type signature of the method.
    pub signature: MethodSignature<T>,
}

impl<T> MethodDescriptor<T> {
    /// Create a new method descriptor.
    pub const fn new(
        id: MethodId,
        name: &'static str,
        param_count: usize,
        param_types: fn(&dyn TypeFactory<T>) -> Vec<T>,
        return_type: fn(&dyn TypeFactory<T>) -> T,
    ) -> Self {
        Self {
            id,
            name,
            signature: MethodSignature {
                param_count,
                param_types,
                return_type,
            },
        }
    }
}

/// Describes an ability that can be registered with the engine.
#[derive(Clone)]
pub struct AbilityDescriptor<T: 'static> {
    /// Ability ID (globally unique).
    pub id: AbilityId,

    /// Ability name as it appears in source code.
    pub name: &'static str,

    /// Methods provided by this ability.
    pub methods: &'static [MethodDescriptor<T>],
}

impl<T> AbilityDescriptor<T> {
    /// Create a new ability descriptor.
    pub const fn new(
        id: AbilityId,
        name: &'static str,
        methods: &'static [MethodDescriptor<T>],
    ) -> Self {
        Self { id, name, methods }
    }

    /// Look up a method by name.
    pub fn get_method(&self, name: &str) -> Option<&MethodDescriptor<T>> {
        self.methods.iter().find(|m| m.name == name)
    }

    /// Look up a method by ID.
    pub fn get_method_by_id(&self, id: MethodId) -> Option<&MethodDescriptor<T>> {
        self.methods.iter().find(|m| m.id == id)
    }
}

/// Trait for types that provide abilities to the engine.
///
/// Implement this trait to define a set of abilities that can be
/// registered with the engine for type checking and compilation.
pub trait AbilityProvider<T> {
    /// Get all ability descriptors provided by this provider.
    fn abilities(&self) -> &[AbilityDescriptor<T>];

    /// Look up an ability by name.
    fn get_ability(&self, name: &str) -> Option<&AbilityDescriptor<T>> {
        self.abilities().iter().find(|a| a.name == name)
    }

    /// Look up an ability by ID.
    fn get_ability_by_id(&self, id: AbilityId) -> Option<&AbilityDescriptor<T>> {
        self.abilities().iter().find(|a| a.id == id)
    }
}
