//! Runtime ability trait for combining type info and handlers.
//!
//! This trait allows host environments to define abilities that provide both
//! compile-time type information (for the type checker) and runtime handlers
//! (for the VM).

use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};

use crate::vm::HostHandler;

/// A runtime ability that provides both compile-time type info and runtime handlers.
///
/// Implementations combine:
/// - Type descriptors for the compiler/type checker
/// - Host handlers for the VM to execute ability methods
///
/// # Example
///
/// ```ignore
/// struct MyAbility;
///
/// impl RuntimeAbility for MyAbility {
///     fn name(&self) -> &'static str { "MyAbility" }
///     fn ability_id(&self) -> AbilityId { 0x1000 }
///     fn descriptor<T: Clone + 'static>(&self, factory: &dyn TypeFactory<T>) -> AbilityDescriptor<T> {
///         // Return type descriptor for compilation
///     }
///     fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
///         // Return handlers for runtime
///     }
/// }
/// ```
pub trait RuntimeAbility: Send + Sync {
    /// The ability name as it appears in source code (e.g., "Console").
    fn name(&self) -> &'static str;

    /// The unique ability ID.
    fn ability_id(&self) -> AbilityId;

    /// Get the ability descriptor for type checking.
    ///
    /// The descriptor contains method signatures used by the compiler
    /// to type-check ability calls.
    fn descriptor<T: Clone + 'static>(&self, factory: &dyn TypeFactory<T>) -> AbilityDescriptor<T>;

    /// Get handlers for all methods in this ability.
    ///
    /// Returns a list of `(method_id, handler)` pairs that will be
    /// registered with the VM.
    fn handlers(&self) -> Vec<(MethodId, HostHandler)>;
}
