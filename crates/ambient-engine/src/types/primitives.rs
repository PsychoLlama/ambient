//! The reserved nominal uuids for built-in types and the [`Primitive`] enum.
//!
//! [`Type`]: super::Type

use uuid::Uuid;

/// Canonical nominal identity of the built-in `Option` enum.
///
/// `Option`/`Result` are reserved-name prelude enums that predate the
/// `unique(<uuid>)` syntax, so they cannot spell their identity in source.
/// They take these fixed, reserved uuids instead. The all-`f` prefix marks
/// them as built-ins and keeps them clear of any real (v4) enum uuid; the
/// low 16 bits are the per-type discriminator, giving the reserved
/// namespace room for 65,535 types.
pub const OPTION_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0001);

/// Canonical nominal identity of the built-in `Result` enum. See [`OPTION_UUID`].
pub const RESULT_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0002);

/// Canonical nominal identity of the built-in `Bool` type. Like
/// `Option`/`Result`, the primitives are reserved-name prelude types homed in
/// `core` that cannot spell their identity in source, so they take fixed
/// reserved uuids in the same `0xffff…` namespace. See [`OPTION_UUID`].
///
/// Two authorities allocate discriminators in this namespace: these Rust
/// consts, and source-declared `unique(...)` types that pick an `0xffff…`
/// uuid by hand (e.g. `core::time::Duration` = `…0003`). To keep the ranges
/// disjoint, the compiler-owned primitives take the *high* end of the
/// discriminator (`0xff00…`); hand-written source uuids stay at the low end.
/// A collision here would be silent: identity unifies on uuid + structure (so
/// structureless `Bool` would not merge with `Duration`), but inherent/ability
/// impl slots key on the uuid *alone*, so `impl Bool` and `Duration`'s methods
/// would land in the same slot.
pub const BOOL_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff01);

/// Canonical nominal identity of the built-in `Number` type. See [`BOOL_UUID`].
pub const NUMBER_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff02);

/// Canonical nominal identity of the built-in `String` type. See [`BOOL_UUID`].
pub const STRING_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff03);

/// Canonical nominal identity of the built-in `Binary` type. See [`BOOL_UUID`].
pub const BINARY_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff04);

/// A built-in primitive type. Primitives are ordinary [`Type::Named`] values
/// carrying a reserved uuid ([`BOOL_UUID`] etc.); this enum is the ergonomic
/// way to match on one, mirroring [`Type::as_option`]/[`Type::as_result`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    /// `Bool`
    Bool,
    /// `Number`
    Number,
    /// `String`
    String,
    /// `Binary`
    Binary,
}

impl Primitive {
    /// The reserved nominal uuid for this primitive.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        match self {
            Self::Bool => BOOL_UUID,
            Self::Number => NUMBER_UUID,
            Self::String => STRING_UUID,
            Self::Binary => BINARY_UUID,
        }
    }

    /// The primitive matching a reserved uuid, if any.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        match uuid {
            BOOL_UUID => Some(Self::Bool),
            NUMBER_UUID => Some(Self::Number),
            STRING_UUID => Some(Self::String),
            BINARY_UUID => Some(Self::Binary),
            _ => None,
        }
    }

    /// The bare type name (e.g. `"String"`), as it renders and is spelled in
    /// source.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "Bool",
            Self::Number => "Number",
            Self::String => "String",
            Self::Binary => "Binary",
        }
    }

    /// The primitive matching a bare type name (e.g. `"String"`), if any. The
    /// name-keyed dual of [`from_uuid`](Self::from_uuid); used by the prelude to
    /// keep the four primitive aliases resolvable in every module.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Bool" => Some(Self::Bool),
            "Number" => Some(Self::Number),
            "String" => Some(Self::String),
            "Binary" => Some(Self::Binary),
            _ => None,
        }
    }

    /// The module-qualified identity (e.g. `"core::primitives::String"`)
    /// surfaced by hover. Primitives are homed in `core::primitives`; the
    /// bare [`name`](Self::name) alone doesn't carry the module.
    #[must_use]
    pub const fn fqn(self) -> &'static str {
        match self {
            Self::Bool => "core::primitives::Bool",
            Self::Number => "core::primitives::Number",
            Self::String => "core::primitives::String",
            Self::Binary => "core::primitives::Binary",
        }
    }
}
