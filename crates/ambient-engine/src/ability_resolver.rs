//! Ability resolver for looking up abilities from registered providers.
//!
//! The `AbilityResolver` aggregates abilities from multiple providers (core, runtime,
//! and any user-defined providers) and provides lookup methods for the type checker
//! and compiler.

use crate::types::Type;
use ambient_core::{AbilityDescriptor, AbilityId, AbilityProvider, MethodId, TypeFactory};
use std::collections::HashMap;
use std::sync::Arc;

/// Resolves ability lookups from registered providers.
///
/// This is used by the type checker and compiler to look up ability and method
/// information without hard-coding the ability definitions.
pub struct AbilityResolver {
    /// Map from ability name to descriptor.
    by_name: HashMap<Arc<str>, AbilityDescriptor<Type>>,

    /// Map from ability ID to descriptor.
    by_id: HashMap<AbilityId, AbilityDescriptor<Type>>,
}

impl AbilityResolver {
    /// Create a new empty ability resolver.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    /// Register abilities from a provider.
    pub fn register<P: AbilityProvider<Type>>(&mut self, provider: &P) {
        for ability in provider.abilities() {
            self.by_name
                .insert(Arc::from(ability.name), ability.clone());
            self.by_id.insert(ability.id, ability.clone());
        }
    }

    /// Look up an ability by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&AbilityDescriptor<Type>> {
        self.by_name.get(name)
    }

    /// Look up an ability by ID.
    #[must_use]
    pub fn get_by_id(&self, id: AbilityId) -> Option<&AbilityDescriptor<Type>> {
        self.by_id.get(&id)
    }

    /// Convert an ability name to its ID.
    #[must_use]
    pub fn name_to_id(&self, name: &str) -> Option<AbilityId> {
        self.by_name.get(name).map(|a| a.id)
    }

    /// Convert an ability ID to its name.
    #[must_use]
    pub fn id_to_name(&self, id: AbilityId) -> Option<&str> {
        self.by_id.get(&id).map(|a| a.name)
    }

    /// Look up a method by ability name and method name.
    #[must_use]
    pub fn get_method(
        &self,
        ability_name: &str,
        method_name: &str,
    ) -> Option<(AbilityId, MethodId)> {
        let ability = self.by_name.get(ability_name)?;
        let method = ability.get_method(method_name)?;
        Some((ability.id, method.id))
    }

    /// Look up a method by ability ID and method name.
    #[must_use]
    pub fn get_method_by_ability_id(
        &self,
        ability_id: AbilityId,
        method_name: &str,
    ) -> Option<MethodId> {
        let ability = self.by_id.get(&ability_id)?;
        let method = ability.get_method(method_name)?;
        Some(method.id)
    }

    /// Get all method signatures for an ability.
    ///
    /// Returns a list of tuples containing method name, parameter count, and return type.
    /// Method names are cloned to avoid lifetime issues.
    #[must_use]
    pub fn get_method_signatures(
        &self,
        ability_id: AbilityId,
        type_factory: &dyn TypeFactory<Type>,
    ) -> Vec<(String, usize, Type)> {
        let Some(ability) = self.by_id.get(&ability_id) else {
            return vec![];
        };

        ability
            .methods
            .iter()
            .map(|m| {
                let return_type = (m.signature.return_type)(type_factory);
                (m.name.to_string(), m.signature.param_count, return_type)
            })
            .collect()
    }

    /// Try to infer which ability a handler literal is for based on method names.
    ///
    /// Returns the ability ID if all methods belong to exactly one ability.
    #[must_use]
    pub fn infer_ability_from_methods(&self, method_names: &[Arc<str>]) -> Option<AbilityId> {
        if method_names.is_empty() {
            return None;
        }

        let mut matching_abilities = Vec::new();

        for ability in self.by_id.values() {
            let ability_methods: Vec<&str> = ability.methods.iter().map(|m| m.name).collect();

            let all_methods_match = method_names
                .iter()
                .all(|m| ability_methods.contains(&m.as_ref()));

            if all_methods_match {
                matching_abilities.push(ability.id);
            }
        }

        // Return only if exactly one ability matches
        if matching_abilities.len() == 1 {
            Some(matching_abilities[0])
        } else {
            None
        }
    }

    /// Get an iterator over all registered abilities.
    pub fn abilities(&self) -> impl Iterator<Item = &AbilityDescriptor<Type>> {
        self.by_id.values()
    }

    /// Get the return type for a method.
    ///
    /// Returns the return type constructed using the provided type factory.
    #[must_use]
    pub fn get_method_return_type(
        &self,
        ability_name: &str,
        method_name: &str,
        type_factory: &dyn TypeFactory<Type>,
    ) -> Option<Type> {
        let ability = self.by_name.get(ability_name)?;
        let method = ability.get_method(method_name)?;
        Some((method.signature.return_type)(type_factory))
    }

    /// Check if a method exists for an ability.
    #[must_use]
    pub fn has_method(&self, ability_name: &str, method_name: &str) -> bool {
        self.by_name
            .get(ability_name)
            .is_some_and(|a| a.get_method(method_name).is_some())
    }
}

impl Default for AbilityResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Type factory implementation for the engine's Type.
pub struct EngineTypeFactory;

impl TypeFactory<Type> for EngineTypeFactory {
    fn unit(&self) -> Type {
        Type::Unit
    }

    fn bool(&self) -> Type {
        Type::Bool
    }

    fn number(&self) -> Type {
        Type::Number
    }

    fn string(&self) -> Type {
        Type::String
    }

    fn never(&self) -> Type {
        Type::Never
    }

    fn type_var(&self) -> Type {
        // For type variables, we return a Hole which will be instantiated
        // during type inference. This is a simplification - in a full
        // implementation we'd track a counter.
        Type::Hole
    }

    fn list(&self, element: Type) -> Type {
        Type::named("List", vec![element])
    }
}

/// Create an `AbilityResolver` with the standard abilities (core + runtime).
#[must_use]
pub fn standard_abilities() -> AbilityResolver {
    let factory = EngineTypeFactory;
    let mut resolver = AbilityResolver::new();

    // Register core abilities
    let core = ambient_core::CoreAbilities::new(&factory);
    resolver.register(&core);

    // Register runtime abilities
    let runtime = ambient_runtime::RuntimeAbilities::new(&factory);
    resolver.register(&runtime);

    resolver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_abilities() {
        let resolver = standard_abilities();

        // Check core abilities
        assert!(resolver.get_by_name("Exception").is_some());

        // Check runtime abilities
        assert!(resolver.get_by_name("Console").is_some());
        assert!(resolver.get_by_name("Time").is_some());
        assert!(resolver.get_by_name("Random").is_some());
        assert!(resolver.get_by_name("Async").is_some());
        assert!(resolver.get_by_name("Log").is_some());
    }

    #[test]
    fn test_ability_lookup() {
        let resolver = standard_abilities();

        // Look up Console.print
        let result = resolver.get_method("Console", "print");
        assert!(result.is_some());
        let (ability_id, method_id) = result.unwrap();
        assert_eq!(ability_id, ambient_runtime::console::ABILITY_ID);
        assert_eq!(method_id, ambient_runtime::console::METHOD_PRINT);
    }

    #[test]
    fn test_infer_ability_from_methods() {
        let resolver = standard_abilities();

        // Methods that match Console
        let methods = vec![Arc::from("print"), Arc::from("println")];
        let result = resolver.infer_ability_from_methods(&methods);
        assert_eq!(result, Some(ambient_runtime::console::ABILITY_ID));

        // Methods that match Exception
        let methods = vec![Arc::from("throw")];
        let result = resolver.infer_ability_from_methods(&methods);
        assert_eq!(result, Some(ambient_core::exception::ABILITY_ID));
    }
}
