//! Handler types and the `RuntimeAbility` trait.

use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};

use crate::error::VmError;
use crate::value::{SuspendedAbility, Value};

/// A host handler for an ability method.
///
/// Host handlers are called when the VM executes an ability operation.
/// They receive the suspended ability (containing the method ID and arguments)
/// and return either a value or an error.
///
/// # Example
///
/// ```ignore
/// let print_handler: HostHandler = Box::new(|ability| {
///     let message = ability.args.first().cloned().unwrap_or(Value::Unit);
///     println!("{}", format_value(&message));
///     Ok(Value::Unit)
/// });
/// ```
pub type HostHandler = Box<dyn Fn(&SuspendedAbility) -> Result<Value, VmError> + Send + Sync>;

/// A runtime ability that provides both compile-time type info and runtime handlers.
///
/// Implementations combine:
/// - Type descriptors for the compiler/type checker
/// - Host handlers for the VM to execute ability methods
///
/// # Example
///
/// ```ignore
/// use ambient_ability::{RuntimeAbility, HostHandler, Value, SuspendedAbility};
/// use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};
///
/// struct MyAbility;
///
/// impl RuntimeAbility for MyAbility {
///     fn name(&self) -> &'static str { "MyAbility" }
///     fn ability_id(&self) -> AbilityId { 0x1000 }
///
///     fn descriptor<T: Clone + 'static>(&self, factory: &dyn TypeFactory<T>) -> AbilityDescriptor<T> {
///         // Return type descriptor for compilation
///         todo!()
///     }
///
///     fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
///         // Return handlers for runtime
///         vec![(0, Box::new(|_| Ok(Value::Unit)))]
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
