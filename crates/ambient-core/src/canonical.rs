//! Canonical ability interface hashing.
//!
//! An ability's identity is the blake3 hash of its *canonical interface*:
//! the ability name plus the ordered list of methods, each with its name
//! and canonicalized parameter/return types. The canonical type encoding
//! is produced by instantiating the ability's [`MethodSignature`] factories
//! with [`CanonicalTypeFactory`], which renders types as stable strings
//! ("unit", "number", "list<string>", ...). Type variables are numbered by
//! order of creation within a signature, so `<T>(T) -> T` always encodes
//! identically regardless of which engine computes it.
//!
//! Changing anything observable about an ability — its name, a method
//! name, method order, arity, or any type in a signature — changes its
//! [`AbilityId`](crate::AbilityId). That is the property remote dispatch
//! relies on: a handler only matches a suspended call if both sides hashed
//! the same interface.

use std::cell::Cell;

use crate::AbilityId;
use crate::descriptor::{MethodDescriptor, TypeFactory};

/// Domain separator for ability interface hashes.
const DOMAIN: &[u8] = b"ambient/ability/v1";

/// A canonical string rendering of a type, produced by [`CanonicalTypeFactory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalType(pub String);

/// Type factory that renders types into their canonical string form.
///
/// Fresh type variables are numbered in creation order (`var0`, `var1`, ...),
/// scoped to this factory instance. Use one factory per signature so
/// variable numbering restarts for each method.
#[derive(Debug, Default)]
pub struct CanonicalTypeFactory {
    next_var: Cell<u32>,
}

impl CanonicalTypeFactory {
    /// Create a factory with variable numbering starting at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TypeFactory<CanonicalType> for CanonicalTypeFactory {
    fn unit(&self) -> CanonicalType {
        CanonicalType("unit".to_string())
    }

    fn bool(&self) -> CanonicalType {
        CanonicalType("bool".to_string())
    }

    fn number(&self) -> CanonicalType {
        CanonicalType("number".to_string())
    }

    fn string(&self) -> CanonicalType {
        CanonicalType("string".to_string())
    }

    fn bytes(&self) -> CanonicalType {
        CanonicalType("bytes".to_string())
    }

    fn never(&self) -> CanonicalType {
        CanonicalType("never".to_string())
    }

    fn type_var(&self) -> CanonicalType {
        let id = self.next_var.get();
        self.next_var.set(id + 1);
        CanonicalType(format!("var{id}"))
    }

    fn list(&self, element: CanonicalType) -> CanonicalType {
        CanonicalType(format!("list<{}>", element.0))
    }
}

/// Write a length-prefixed string into the hasher.
fn write_str(hasher: &mut blake3::Hasher, s: &str) {
    #[allow(clippy::cast_possible_truncation)]
    let len = s.len() as u32;
    hasher.update(&len.to_le_bytes());
    hasher.update(s.as_bytes());
}

/// One method of an ability interface in already-canonicalized form.
///
/// This is the byte-level input to interface hashing: producers that
/// don't go through [`MethodSignature`] factories (e.g. in-language
/// `ability` declarations, whose types come from the type checker) render
/// their signatures to canonical strings and hash through this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMethod {
    /// Declaration index of the method within the ability.
    pub id: u16,
    /// Method name as written in source.
    pub name: String,
    /// Canonical renderings of the parameter types, in order.
    pub params: Vec<String>,
    /// Canonical rendering of the return type.
    pub ret: String,
}

/// Compute the content-addressed identity of an ability interface from
/// pre-rendered canonical signatures.
///
/// Methods are hashed in method-ID order (the declaration index), so the
/// hash commits to the `(MethodId, name, signature)` mapping regardless of
/// the order methods appear in the input. Combined with the ability name,
/// `(AbilityId, MethodId)` is globally unambiguous.
#[must_use]
pub fn hash_interface_raw(name: &str, methods: &[RawMethod]) -> AbilityId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    write_str(&mut hasher, name);

    let mut methods: Vec<&RawMethod> = methods.iter().collect();
    methods.sort_by_key(|m| m.id);

    #[allow(clippy::cast_possible_truncation)]
    let count = methods.len() as u32;
    hasher.update(&count.to_le_bytes());

    for method in methods {
        hasher.update(&method.id.to_le_bytes());
        write_str(&mut hasher, &method.name);

        #[allow(clippy::cast_possible_truncation)]
        let param_count = method.params.len() as u32;
        hasher.update(&param_count.to_le_bytes());
        for param in &method.params {
            write_str(&mut hasher, param);
        }
        write_str(&mut hasher, &method.ret);
    }

    AbilityId::from_bytes(*hasher.finalize().as_bytes())
}

/// Compute the content-addressed identity of an ability interface.
///
/// Renders each signature with a fresh [`CanonicalTypeFactory`]
/// (variable numbering is signature-local) and hashes via
/// [`hash_interface_raw`].
#[must_use]
pub fn hash_interface(name: &str, methods: &[MethodDescriptor<CanonicalType>]) -> AbilityId {
    let raw: Vec<RawMethod> = methods
        .iter()
        .map(|method| {
            let factory = CanonicalTypeFactory::new();
            let params = (method.signature.param_types)(&factory);
            let ret = (method.signature.return_type)(&factory);
            RawMethod {
                id: method.id,
                name: method.name.to_string(),
                params: params.into_iter().map(|p| p.0).collect(),
                ret: ret.0,
            }
        })
        .collect();
    hash_interface_raw(name, &raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::MethodDescriptor;

    fn print_method() -> MethodDescriptor<CanonicalType> {
        MethodDescriptor::new(0, "print", 1, |f| vec![f.string()], |f| f.unit())
    }

    #[test]
    fn deterministic() {
        let a = hash_interface("Console", &[print_method()]);
        let b = hash_interface("Console", &[print_method()]);
        assert_eq!(a, b);
    }

    #[test]
    fn name_changes_identity() {
        let a = hash_interface("Console", &[print_method()]);
        let b = hash_interface("Terminal", &[print_method()]);
        assert_ne!(a, b);
    }

    #[test]
    fn signature_changes_identity() {
        let a = hash_interface("Console", &[print_method()]);
        let b = hash_interface(
            "Console",
            &[MethodDescriptor::new(
                0,
                "print",
                1,
                |f| vec![f.number()],
                |f| f.unit(),
            )],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn array_order_is_canonicalized() {
        // Methods are hashed in method-ID order, so descriptor array order
        // is irrelevant...
        let read = || MethodDescriptor::new(0, "read", 1, |f| vec![f.string()], |f| f.string());
        let write = || MethodDescriptor::new(1, "write", 1, |f| vec![f.string()], |f| f.unit());
        let a = hash_interface("FileSystem", &[read(), write()]);
        let b = hash_interface("FileSystem", &[write(), read()]);
        assert_eq!(a, b);
    }

    #[test]
    fn method_id_mapping_changes_identity() {
        // ...but which ID maps to which method is part of the identity.
        let a = hash_interface(
            "FileSystem",
            &[
                MethodDescriptor::new(0, "read", 1, |f| vec![f.string()], |f| f.string()),
                MethodDescriptor::new(1, "write", 1, |f| vec![f.string()], |f| f.unit()),
            ],
        );
        let b = hash_interface(
            "FileSystem",
            &[
                MethodDescriptor::new(1, "read", 1, |f| vec![f.string()], |f| f.string()),
                MethodDescriptor::new(0, "write", 1, |f| vec![f.string()], |f| f.unit()),
            ],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn type_vars_number_per_signature() {
        // <T>(T) -> T should hash identically no matter how many other
        // signatures were rendered before it.
        let identity =
            || MethodDescriptor::new(0, "id", 1, |f| vec![f.type_var()], |f| f.type_var());
        let a = hash_interface("Id", &[identity()]);
        let b = hash_interface("Id", &[identity()]);
        assert_eq!(a, b);
    }
}
