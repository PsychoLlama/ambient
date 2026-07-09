//! Ability dispatch types: compiled method references, suspended
//! performs, and first-class handler values.

use std::collections::HashMap;

use ambient_core::{AbilityId, MethodKey, SignatureHash};
use serde::{Deserialize, Serialize};

use crate::value::Value;

/// A compiled reference to one ability method.
///
/// This is the constant-pool shape behind every perform site and handler
/// arm. It carries the three inputs a [`MethodKey`] derives from — the
/// ability's declaration uuid, the canonical signature hash, and the
/// default implementation's content hash — plus the uuid-derived
/// [`AbilityId`] for dispatch. The key itself is derived on demand
/// ([`Self::method_key`]) rather than stored, so it can never disagree
/// with its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbilityMethodRef {
    /// The uuid-derived identity of the ability.
    pub ability_id: AbilityId,
    /// The ability's declaration uuid (a `MethodKey` input).
    pub ability_uuid: uuid::Uuid,
    /// The method's canonical signature hash (a `MethodKey` input).
    pub signature: SignatureHash,
    /// Content hash of the method's default implementation — the function
    /// an unhandled perform calls. `None` only for the abstract
    /// `Exception::throw`, whose unhandled behavior is the VM's own
    /// uncaught-exception path.
    pub impl_fn: Option<blake3::Hash>,
}

impl AbilityMethodRef {
    /// Derive the method's content-addressed identity.
    #[must_use]
    pub fn method_key(&self) -> MethodKey {
        MethodKey::derive(
            &self.ability_uuid,
            &self.signature,
            self.impl_fn.as_ref().map(blake3::Hash::as_bytes),
        )
    }
}

/// A suspended ability operation waiting to be performed.
///
/// This type is fully serializable, allowing ability values to be stored,
/// transmitted, and executed remotely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendedAbility {
    /// The uuid-derived identity of the ability being invoked.
    pub ability_id: AbilityId,

    /// The content-addressed identity of the method being invoked.
    pub method: MethodKey,

    /// Content hash of the method's default implementation, which an
    /// unhandled perform calls (`None` for the `Exception` carve-out).
    pub impl_fn: Option<blake3::Hash>,

    /// The arguments to pass to the ability method.
    pub args: Vec<Value>,
}

impl SuspendedAbility {
    /// Create a new suspended ability.
    #[must_use]
    pub fn new(ability_id: AbilityId, method: MethodKey, args: Vec<Value>) -> Self {
        Self {
            ability_id,
            method,
            impl_fn: None,
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
/// let mock_fs: Handler<FileSystem> = {
///   FileSystem::read(path) => resume("mock content"),
///   FileSystem::write(path, content) => resume(()),
///   FileSystem::exists(path) => resume(true),
/// };
/// ```
///
/// They can be composed with other handlers and used in handle expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerValue {
    /// The uuid-derived identity of the ability this handler handles.
    pub ability_id: AbilityId,

    /// Method implementations: method key -> function hash of the arm.
    /// Each handler function receives implicit parameters: (continuation, `suspended_ability`)
    /// and can extract ability arguments from the suspended ability.
    pub methods: HashMap<MethodKey, blake3::Hash>,

    /// Optional captured environment for closures within the handler.
    /// If handler methods capture variables from their surrounding scope,
    /// those values are stored here.
    pub captures: Vec<Value>,
}

impl HandlerValue {
    /// Create a new handler value.
    #[must_use]
    pub fn new(ability_id: AbilityId, methods: HashMap<MethodKey, blake3::Hash>) -> Self {
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
        methods: HashMap<MethodKey, blake3::Hash>,
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
    pub fn get_method(&self, method: MethodKey) -> Option<blake3::Hash> {
        self.methods.get(&method).copied()
    }

    /// Check if this handler handles a specific method.
    #[must_use]
    pub fn handles_method(&self, method: MethodKey) -> bool {
        self.methods.contains_key(&method)
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
