//! Type system for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with support for:
//! - Primitive types (number, string, bool, unit)
//! - Composite types (tuples, records, functions)
//! - Polymorphic types (generics with type variables)
//! - Nominal types (unique types distinguished by UUID)
//! - Ability types for tracking side effects (Milestone 8)
//!
//! The type system uses structural equivalence by default, with nominal
//! types providing opt-in name-based distinction.
//!
//! # Module organization
//!
//! Every item is re-exported flat, so consumers keep using
//! `crate::types::X`:
//!
//! - `core.rs` - the [`Type`] enum, its support structs ([`RecordType`],
//!   [`FunctionType`], ...), [`TypeVarGen`], and convenience constructors
//! - `primitives.rs` - the [`Primitive`] and [`Container`] enums and the
//!   reserved uuids ([`BOOL_UUID`], [`OPTION_UUID`], [`LIST_UUID`], ...)
//! - `abilities.rs` - [`AbilitySet`], [`AbilityInfo`], [`AbilityRegistry`]
//! - `traits.rs` - the trait system ([`TraitDef`], [`TraitRegistry`], ...)
//!   plus the [`impl_method_symbol`]/[`uuid_to_source`] symbol helpers
//! - `ops.rs` - type algorithms: substitution, free variables, concreteness
//! - `display.rs` - `Display` impls for [`Type`] and [`AbilitySet`]

mod abilities;
mod core;
mod display;
mod ops;
mod primitives;
#[cfg(test)]
mod tests;
mod traits;

/// A unique identifier for type variables, used during unification.
pub type TypeVarId = u32;

/// A unique identifier for ability variables, used during ability inference.
pub type AbilityVarId = u32;

/// An ability identifier: the content-addressed identity of the ability's
/// canonical interface (re-exported from `ambient-core`).
pub use ambient_core::AbilityId;

/// A unique identifier for traits.
pub type TraitId = u16;

pub use abilities::{AbilityInfo, AbilityRegistry, AbilitySet};
pub use core::{
    AbilityValueType, ForallType, FunctionType, HandlerType, NamedType, NominalType, RecordType,
    Type, TypeVarGen,
};
pub use primitives::{
    BINARY_UUID, BOOL_UUID, Container, LIST_UUID, MAP_UUID, NUMBER_UUID, OPTION_UUID, Primitive,
    RESULT_UUID, SET_UUID, STRING_UUID,
};
pub use traits::{
    MethodLookup, TraitDef, TraitImpl, TraitMethodDef, TraitRegistry, impl_method_symbol,
    uuid_to_source,
};
