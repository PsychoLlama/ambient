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
/// disjoint, the compiler-owned built-ins (the scalar primitives *and* the
/// generic containers `List`/`Map`/`Set`) take the *high* end of the
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

/// Canonical nominal identity of the built-in `List<T>` container. Like the
/// scalar primitives, the containers are reserved-name generic types homed in
/// `core::collections` and declared in Ambient source as `extern` unit structs
/// (`extern unique(...) struct List<T>;`), anchored on these reserved uuids.
/// A container's phantom type parameter never appears in its (empty) body, so
/// its applied form is a [`Type::Named`] carrying the uuid + arguments — the
/// same shape as `Option`/`Result` — rather than the field-substituting
/// `Type::Nominal` a scalar primitive lowers to. See [`BOOL_UUID`].
pub const LIST_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff05);

/// Canonical nominal identity of the built-in `Map<K, V>` container. See
/// [`LIST_UUID`].
pub const MAP_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff06);

/// Canonical nominal identity of the built-in `Set<T>` container. See
/// [`LIST_UUID`].
pub const SET_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ff07);

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

    /// The module-qualified identity (e.g. `"core::primitives::string"`)
    /// surfaced by hover. Primitives are homed in `core::primitives`; the
    /// bare [`name`](Self::name) alone doesn't carry the module.
    #[must_use]
    pub const fn fqn(self) -> &'static str {
        match self {
            Self::Bool => "core::primitives::bool",
            Self::Number => "core::primitives::number",
            Self::String => "core::primitives::string",
            Self::Binary => "core::primitives::binary",
        }
    }
}

/// A built-in generic container type (`List`/`Map`/`Set`). Containers are the
/// generic counterpart to [`Primitive`]: reserved-name types carrying a fixed
/// nominal uuid ([`LIST_UUID`] etc.), but with a phantom type parameter, so
/// their applied form is a [`Type::Named`] with arguments (like `Option`) not a
/// field-substituting [`Type::Nominal`]. This enum is the ergonomic way to map
/// a head name to its reserved identity — the container analogue of the
/// name/uuid tables `Primitive` exposes.
///
/// [`Type::Named`]: super::Type
/// [`Type::Nominal`]: super::Type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// `List<T>`
    List,
    /// `Map<K, V>`
    Map,
    /// `Set<T>`
    Set,
}

impl Container {
    /// The reserved nominal uuid for this container.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        match self {
            Self::List => LIST_UUID,
            Self::Map => MAP_UUID,
            Self::Set => SET_UUID,
        }
    }

    /// The container matching a reserved uuid, if any.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        match uuid {
            LIST_UUID => Some(Self::List),
            MAP_UUID => Some(Self::Map),
            SET_UUID => Some(Self::Set),
            _ => None,
        }
    }

    /// The bare type name (e.g. `"List"`), as it renders and is spelled in
    /// source.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Map => "Map",
            Self::Set => "Set",
        }
    }

    /// The container matching a bare head name (e.g. `"List"`), if any. The
    /// name-keyed dual of [`from_uuid`](Self::from_uuid); the sole point the
    /// checker recognizes a container head, so `List<T>` resolves to its
    /// reserved identity in every module without an import.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "List" => Some(Self::List),
            "Map" => Some(Self::Map),
            "Set" => Some(Self::Set),
            _ => None,
        }
    }

    /// The number of type parameters the container takes (`List<T>` → 1,
    /// `Map<K, V>` → 2). Pins the canonical arity its `extern` declaration must
    /// match.
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::List | Self::Set => 1,
            Self::Map => 2,
        }
    }
}
