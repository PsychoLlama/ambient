//! Canonical binary encoding for content-addressed objects.
//!
//! This module defines THE identity of Ambient code: a function's hash is the
//! blake3 hash of its canonical encoding. The same encoding is used for the
//! on-disk store, the wire protocol, and hash computation, so an object is
//! self-verifying everywhere: `blake3(bytes) == expected hash`, always.
//!
//! # Object kinds
//!
//! - **Plain** — a single non-recursive function. All function references are
//!   external (final hashes). `hash = blake3(encoding)`.
//! - **Group** — a strongly connected component of mutually (or self-)
//!   recursive functions, stored as one unit. References between members are
//!   encoded as member *indices*, not hashes, which breaks the circularity
//!   that would otherwise make recursive functions un-hashable.
//!   `group_hash = blake3(encoding)`; each member's function hash is derived:
//!   the group hash itself for a single-member group, otherwise
//!   `blake3("ambient/member/v1" ‖ group_hash ‖ index)`.
//! - **Redirect** — a small pointer stored *at a member's hash* that says
//!   "this function is member `i` of group `g`". Redirects are not
//!   self-hashing; they are verified by loading the group and re-deriving the
//!   member hash.
//!
//! # Canonical member ordering
//!
//! Group members are ordered canonically so that identity does not depend on
//! compilation order:
//!
//! 1. Named members first, sorted by name. (Names of recursive functions are
//!    part of their group's identity — renaming a member of a recursive group
//!    changes its hash. This is deliberate: the members of a cycle are only
//!    distinguishable by name.)
//! 2. Lambda members (unnamed) follow, in the order they are first referenced
//!    during an in-order scan of the constant pools of already-ordered
//!    members. Every lambda in a cycle is reachable from a named member of
//!    that cycle, so this yields a total, deterministic order.
//!
//! # Encoding layout (all integers little-endian)
//!
//! - **Value** — a single content-addressed `const` value (a `Unit`, `Bool`,
//!   `Number`, `String`, or `Binary`). Its hash is a pure function of the
//!   value's type tag and bytes, independent of the const's name, so two
//!   consts with the same value share one object. Value objects are leaves:
//!   they reference no other object.
//!
//! ```text
//! header:   magic "ABOB" | version u8 = 1 | kind u8 (0 plain, 1 group, 2 redirect, 3 value)
//! plain:    body
//! group:    member_count u32 | members: [has_name u8, (name_len u32, name)?, body]
//! redirect: group_hash [32] | index u32
//! value:    constant
//! body:     bytecode_len u32 | bytecode
//!           local_count u16 | param_count u8
//!           const_count u32 | constants
//!           dep_count u32 | refs
//! ref:      0 u8 | hash [32]        (external)
//!           1 u8 | index u32        (internal to the group)
//! constant: 0 unit | 1 bool u8 | 2 number f64-bits | 3 string (u32, utf8)
//!           4 bytes (u32, raw) | 5 ref | 6 ability [32] | 7 value-ref [32]
//!           8 ability-method: ability [32] | uuid [16] | sig [32] | has_impl u8 | ref?
//! ```
//!
//! Decoding rejects trailing bytes and unknown tags, so decode∘encode is the
//! identity and every byte of an object is covered by its hash.

// Length prefixes are u32 by design: no single function's bytecode, constant
// pool, or member list can meaningfully exceed 2^32 entries, and fixed-width
// prefixes keep the encoding canonical.
#![allow(clippy::cast_possible_truncation)]

use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::value::Value;

/// Magic bytes identifying an Ambient object.
pub const OBJECT_MAGIC: [u8; 4] = *b"ABOB";

/// Current object encoding version.
///
/// v2: constant pools may contain ability references (tag 6), the 32-byte
/// content hash of an ability interface.
///
/// v3: handle expressions compile to body thunks; the `Handle` instruction
/// pops its arm closure and carries only the ability constant index, and
/// `HandleWithValue` has no operands. Bytecode from earlier versions
/// decodes differently and must not be executed.
///
/// Still v3: the `Native` kind (tag 4, an `extern fn`'s identity) was added
/// without a version bump — the addition is purely new (no existing kind's
/// bytes or meaning changed, so no existing hash moved), and older engines
/// reject the unknown kind tag rather than misreading it.
///
/// v4: abilities are nominal. Constant pools carry ability-method
/// references (tag 8: ability id, declaration uuid, canonical signature
/// hash, and the default implementation's ref), and the `Suspend`
/// instruction's operands changed from (ability const, method u16, argc)
/// to (method const, argc) — bytecode from earlier versions decodes
/// differently and must not be executed.
pub const OBJECT_VERSION: u8 = 4;

const KIND_PLAIN: u8 = 0;
const KIND_GROUP: u8 = 1;
const KIND_REDIRECT: u8 = 2;
const KIND_VALUE: u8 = 3;
const KIND_NATIVE: u8 = 4;

const REF_EXTERNAL: u8 = 0;
const REF_INTERNAL: u8 = 1;

const CONST_UNIT: u8 = 0;
const CONST_BOOL: u8 = 1;
const CONST_NUMBER: u8 = 2;
const CONST_STRING: u8 = 3;
const CONST_BYTES: u8 = 4;
const CONST_REF: u8 = 5;
const CONST_ABILITY: u8 = 6;
const CONST_VALUEREF: u8 = 7;
const CONST_ABILITY_METHOD: u8 = 8;

/// A reference to another function, from inside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRef {
    /// A finalized content hash of a function outside this object.
    External(blake3::Hash),
    /// An index into this object's member list (recursive groups only).
    Internal(u32),
}

/// A constant-pool entry in canonical form.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectConstant {
    Unit,
    Bool(bool),
    Number(f64),
    String(String),
    Binary(Vec<u8>),
    Ref(ObjectRef),
    /// The content-addressed identity of an ability interface.
    Ability(ambient_core::AbilityId),
    /// The content hash of a `const` value object. Distinct from [`Self::Ref`]
    /// (a function reference) so a value ref is never mistaken for a function
    /// ref: it decodes to [`Value::ObjectRef`], not [`Value::FunctionRef`].
    /// Value objects are leaves, so this is always an external final hash —
    /// never an internal group index.
    ValueRef(blake3::Hash),
    /// One ability method, as perform sites and handler arms reference it:
    /// the uuid-derived ability id, the declaration uuid and canonical
    /// signature hash (the `MethodKey` inputs), and the default
    /// implementation's function reference (`None` only for the abstract
    /// `Exception::throw`). The implementation is an [`ObjectRef`] like any
    /// callee, so a method whose impl lands in the same recursive group
    /// encodes by member index.
    AbilityMethod {
        ability: ambient_core::AbilityId,
        uuid: uuid::Uuid,
        signature: ambient_core::SignatureHash,
        impl_fn: Option<ObjectRef>,
    },
}

/// The body of one function inside an object.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectFunction {
    pub bytecode: Vec<u8>,
    pub constants: Vec<ObjectConstant>,
    pub local_count: u16,
    pub param_count: u8,
    pub dependencies: Vec<ObjectRef>,
}

/// One member of a recursive group.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupMember {
    /// Source name for named functions; `None` for lambdas.
    pub name: Option<String>,
    pub function: ObjectFunction,
}

/// A content-addressed object: the unit of storage and exchange.
#[derive(Debug, Clone, PartialEq)]
pub enum StoredObject {
    /// A single non-recursive function.
    Plain(ObjectFunction),
    /// A strongly connected component of recursive functions.
    Group(Vec<GroupMember>),
    /// A pointer from a member hash to its group.
    Redirect { group: blake3::Hash, index: u32 },
    /// A single content-addressed `const` value. Its hash is a pure function
    /// of the value (type tag + bytes), independent of the const's name, so
    /// identical values deduplicate to one object.
    Value(ObjectConstant),
    /// An `extern fn`: a host-implemented function identified by a stable
    /// UUID. The encoding is exactly `(uuid, param_count)` — never the
    /// declared name — so renaming an extern fn moves no hash, while the
    /// UUID makes two different natives unmistakable. Callers link to a
    /// native's hash exactly like any function, so packs ship it in the
    /// dependency closure; the receiving VM binds the UUID against its own
    /// native table or fails loudly
    /// ([`VmError::UnboundNative`](crate::VmError::UnboundNative)).
    /// Like value objects, natives are leaves: they reference nothing.
    Native { uuid: uuid::Uuid, param_count: u8 },
}

/// Errors from encoding, decoding, or materializing objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectError {
    /// A constant pool value cannot be represented canonically.
    UnsupportedConstant(&'static str),
    /// Input ended before the encoding was complete.
    Truncated,
    /// Input did not start with the object magic.
    BadMagic,
    /// Unknown encoding version.
    BadVersion(u8),
    /// Unknown kind/ref/constant tag.
    BadTag(u8),
    /// Binary remained after a complete object was decoded.
    TrailingBytes,
    /// A string constant or member name was not valid UTF-8.
    InvalidUtf8,
    /// An internal reference pointed past the end of the member list.
    InternalRefOutOfRange { index: u32, member_count: u32 },
    /// A plain object contained an internal reference.
    InternalRefInPlain,
    /// Attempted to materialize functions from a redirect.
    MaterializeRedirect,
    /// A group object had no members.
    EmptyGroup,
}

impl std::fmt::Display for ObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedConstant(kind) => {
                write!(f, "constant of kind `{kind}` cannot be content-addressed")
            }
            Self::Truncated => write!(f, "object encoding is truncated"),
            Self::BadMagic => write!(f, "not an Ambient object (bad magic)"),
            Self::BadVersion(v) => write!(f, "unsupported object version {v}"),
            Self::BadTag(t) => write!(f, "unknown tag {t} in object encoding"),
            Self::TrailingBytes => write!(f, "trailing bytes after object encoding"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in object encoding"),
            Self::InternalRefOutOfRange {
                index,
                member_count,
            } => write!(
                f,
                "internal reference to member {index} but group has {member_count} members"
            ),
            Self::InternalRefInPlain => {
                write!(f, "plain object contains an internal (group) reference")
            }
            Self::MaterializeRedirect => {
                write!(f, "redirect objects carry no code; load the group instead")
            }
            Self::EmptyGroup => write!(f, "group object has no members"),
        }
    }
}

impl std::error::Error for ObjectError {}

// ─────────────────────────────────────────────────────────────────────────────
// Hash derivation
// ─────────────────────────────────────────────────────────────────────────────

/// Compute the hash of a group member's function.
///
/// Single-member groups (self-recursive functions) use the group hash
/// directly; multi-member groups derive per-member hashes from the group
/// hash and the member index.
#[must_use]
pub fn member_hash(group_hash: &blake3::Hash, index: u32, member_count: u32) -> blake3::Hash {
    if member_count == 1 {
        return *group_hash;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ambient/member/v1");
    hasher.update(group_hash.as_bytes());
    hasher.update(&index.to_le_bytes());
    hasher.finalize()
}

impl StoredObject {
    /// The content hash of this object: blake3 over the canonical encoding.
    ///
    /// For groups this is the *group* hash; member function hashes are
    /// derived from it via [`member_hash`]. Redirects are not
    /// content-addressed by their own bytes; calling this on a redirect
    /// hashes its encoding, which is only useful for testing.
    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        blake3::hash(&self.encode())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Encoding
    // ─────────────────────────────────────────────────────────────────────

    /// Encode this object to its canonical byte representation.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&OBJECT_MAGIC);
        out.push(OBJECT_VERSION);
        match self {
            Self::Plain(func) => {
                out.push(KIND_PLAIN);
                encode_function(&mut out, func);
            }
            Self::Group(members) => {
                out.push(KIND_GROUP);
                out.extend_from_slice(&(members.len() as u32).to_le_bytes());
                for member in members {
                    match &member.name {
                        Some(name) => {
                            out.push(1);
                            out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                            out.extend_from_slice(name.as_bytes());
                        }
                        None => out.push(0),
                    }
                    encode_function(&mut out, &member.function);
                }
            }
            Self::Redirect { group, index } => {
                out.push(KIND_REDIRECT);
                out.extend_from_slice(group.as_bytes());
                out.extend_from_slice(&index.to_le_bytes());
            }
            Self::Value(constant) => {
                out.push(KIND_VALUE);
                encode_constant(&mut out, constant);
            }
            Self::Native { uuid, param_count } => {
                out.push(KIND_NATIVE);
                out.extend_from_slice(uuid.as_bytes());
                out.push(*param_count);
            }
        }
        out
    }

    /// Decode an object from its canonical byte representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a complete, well-formed object.
    pub fn decode(bytes: &[u8]) -> Result<Self, ObjectError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(4)? != OBJECT_MAGIC {
            return Err(ObjectError::BadMagic);
        }
        let version = r.u8()?;
        if version != OBJECT_VERSION {
            return Err(ObjectError::BadVersion(version));
        }
        let kind = r.u8()?;
        let object = match kind {
            KIND_PLAIN => {
                let func = decode_function(&mut r)?;
                if func_has_internal_refs(&func) {
                    return Err(ObjectError::InternalRefInPlain);
                }
                Self::Plain(func)
            }
            KIND_GROUP => {
                let count = r.u32()?;
                if count == 0 {
                    return Err(ObjectError::EmptyGroup);
                }
                // Cap pre-allocation by input size: each member costs at
                // least one byte, so a lying count can't force a huge alloc.
                let mut members = Vec::with_capacity((count as usize).min(r.remaining()));
                for _ in 0..count {
                    let name = match r.u8()? {
                        0 => None,
                        1 => {
                            let len = r.u32()? as usize;
                            let raw = r.take(len)?;
                            Some(
                                std::str::from_utf8(raw)
                                    .map_err(|_| ObjectError::InvalidUtf8)?
                                    .to_string(),
                            )
                        }
                        t => return Err(ObjectError::BadTag(t)),
                    };
                    let function = decode_function(&mut r)?;
                    members.push(GroupMember { name, function });
                }
                for member in &members {
                    check_internal_refs(&member.function, count)?;
                }
                Self::Group(members)
            }
            KIND_REDIRECT => {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(r.take(32)?);
                let index = r.u32()?;
                Self::Redirect {
                    group: blake3::Hash::from_bytes(hash_bytes),
                    index,
                }
            }
            KIND_VALUE => Self::Value(decode_constant(&mut r)?),
            KIND_NATIVE => {
                let mut uuid_bytes = [0u8; 16];
                uuid_bytes.copy_from_slice(r.take(16)?);
                let param_count = r.u8()?;
                Self::Native {
                    uuid: uuid::Uuid::from_bytes(uuid_bytes),
                    param_count,
                }
            }
            t => return Err(ObjectError::BadTag(t)),
        };
        if r.pos != bytes.len() {
            return Err(ObjectError::TrailingBytes);
        }
        Ok(object)
    }

    // ─────────────────────────────────────────────────────────────────────
    // Materialization
    // ─────────────────────────────────────────────────────────────────────

    /// The runtime value of a value object, or `None` for code objects.
    ///
    /// Value objects hold a single `const` value; every function-reference
    /// pool entry is external (value objects are leaves), so decoding never
    /// needs a group context.
    #[must_use]
    pub fn as_value(&self) -> Option<Value> {
        match self {
            Self::Value(constant) => Some(constant_to_value(constant)),
            _ => None,
        }
    }

    /// The `(uuid, param_count)` identity of a native object, or `None`
    /// for every other kind.
    #[must_use]
    pub fn as_native(&self) -> Option<(uuid::Uuid, u8)> {
        match self {
            Self::Native { uuid, param_count } => Some((*uuid, *param_count)),
            _ => None,
        }
    }

    /// Turn this object into runnable functions with final hashes.
    ///
    /// Returns `(hash, function)` pairs: one for a plain object, one per
    /// member for a group. Value objects carry no code and yield an empty
    /// list. All internal references are substituted with the derived member
    /// hashes.
    ///
    /// # Errors
    ///
    /// Returns an error for redirects (they carry no code) or malformed
    /// internal references.
    pub fn materialize(&self) -> Result<Vec<(blake3::Hash, CompiledFunction)>, ObjectError> {
        match self {
            // Values and natives carry no bytecode. A native's runtime form
            // is a host implementation, resolved by UUID at the VM (see
            // `Vm::load_native`).
            Self::Value(_) | Self::Native { .. } => Ok(Vec::new()),
            Self::Plain(func) => {
                let hash = self.hash();
                let compiled = to_compiled(func, hash, &|_| None)?;
                Ok(vec![(hash, compiled)])
            }
            Self::Group(members) => {
                let group = self.hash();
                let count = members.len() as u32;
                let hashes: Vec<blake3::Hash> =
                    (0..count).map(|i| member_hash(&group, i, count)).collect();
                members
                    .iter()
                    .enumerate()
                    .map(|(i, member)| {
                        let compiled = to_compiled(&member.function, hashes[i], &|idx| {
                            hashes.get(idx as usize).copied()
                        })?;
                        Ok((hashes[i], compiled))
                    })
                    .collect()
            }
            Self::Redirect { .. } => Err(ObjectError::MaterializeRedirect),
        }
    }
}

/// Convert a runtime constant-pool value to canonical form.
///
/// `resolve` classifies function references as internal (to the object being
/// built) or external.
///
/// # Errors
///
/// Returns an error for value kinds that cannot appear in canonical objects.
pub fn constant_from_value(
    value: &Value,
    resolve: &dyn Fn(&blake3::Hash) -> ObjectRef,
) -> Result<ObjectConstant, ObjectError> {
    Ok(match value {
        Value::Unit => ObjectConstant::Unit,
        Value::Bool(b) => ObjectConstant::Bool(*b),
        Value::Number(n) => ObjectConstant::Number(*n),
        Value::String(s) => ObjectConstant::String((**s).clone()),
        Value::Binary(b) => ObjectConstant::Binary((**b).clone()),
        Value::FunctionRef(h) => ObjectConstant::Ref(resolve(h)),
        // A value ref is always a final external hash (value objects are
        // leaves), so it bypasses `resolve`, which only classifies function
        // references as internal/external.
        Value::ObjectRef(h) => ObjectConstant::ValueRef(*h),
        Value::AbilityRef(id) => ObjectConstant::Ability(*id),
        Value::AbilityMethodRef(m) => ObjectConstant::AbilityMethod {
            ability: m.ability_id,
            uuid: m.ability_uuid,
            signature: m.signature,
            impl_fn: m.impl_fn.as_ref().map(resolve),
        },
        Value::Tuple(_) => return Err(ObjectError::UnsupportedConstant("tuple")),
        Value::Record(_) => return Err(ObjectError::UnsupportedConstant("record")),
        Value::List(_) => return Err(ObjectError::UnsupportedConstant("list")),
        Value::Map(_) => return Err(ObjectError::UnsupportedConstant("map")),
        Value::Set(_) => return Err(ObjectError::UnsupportedConstant("set")),
        Value::Enum(_) => return Err(ObjectError::UnsupportedConstant("enum")),
        Value::Closure(_) => return Err(ObjectError::UnsupportedConstant("closure")),
        Value::Handler(_) => return Err(ObjectError::UnsupportedConstant("handler")),
        Value::Continuation(_) => return Err(ObjectError::UnsupportedConstant("continuation")),
        Value::SuspendedAbility(_) => {
            return Err(ObjectError::UnsupportedConstant("suspended ability"));
        }
        Value::Module(_) => return Err(ObjectError::UnsupportedConstant("module")),
        Value::ModuleMember(_) => return Err(ObjectError::UnsupportedConstant("module member")),
    })
}

/// Build an [`ObjectFunction`] from a compiled function.
///
/// `resolve` maps every function-reference hash (in constants and the
/// dependency list) to an internal or external reference.
///
/// # Errors
///
/// Returns an error if the constant pool contains non-encodable values.
pub fn function_from_compiled(
    func: &CompiledFunction,
    resolve: &dyn Fn(&blake3::Hash) -> ObjectRef,
) -> Result<ObjectFunction, ObjectError> {
    Ok(ObjectFunction {
        bytecode: func.bytecode.clone(),
        constants: func
            .constants
            .iter()
            .map(|v| constant_from_value(v, resolve))
            .collect::<Result<_, _>>()?,
        local_count: func.local_count,
        param_count: func.param_count,
        dependencies: func.dependencies.iter().map(resolve).collect(),
    })
}

/// Reconstruct a runnable function from an object body.
fn to_compiled(
    func: &ObjectFunction,
    hash: blake3::Hash,
    internal: &dyn Fn(u32) -> Option<blake3::Hash>,
) -> Result<CompiledFunction, ObjectError> {
    let resolve = |r: &ObjectRef| -> Result<blake3::Hash, ObjectError> {
        match r {
            ObjectRef::External(h) => Ok(*h),
            ObjectRef::Internal(i) => internal(*i).ok_or(ObjectError::InternalRefOutOfRange {
                index: *i,
                member_count: 0,
            }),
        }
    };

    let constants = func
        .constants
        .iter()
        .map(|c| {
            Ok(match c {
                ObjectConstant::Ref(r) => Value::FunctionRef(resolve(r)?),
                ObjectConstant::AbilityMethod {
                    ability,
                    uuid,
                    signature,
                    impl_fn,
                } => Value::AbilityMethodRef(Arc::new(crate::value::AbilityMethodRef {
                    ability_id: *ability,
                    ability_uuid: *uuid,
                    signature: *signature,
                    impl_fn: impl_fn.as_ref().map(&resolve).transpose()?,
                })),
                other => constant_to_value(other),
            })
        })
        .collect::<Result<Vec<_>, ObjectError>>()?;

    let dependencies = func
        .dependencies
        .iter()
        .map(&resolve)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CompiledFunction {
        hash,
        bytecode: func.bytecode.clone(),
        constants,
        local_count: func.local_count,
        param_count: func.param_count,
        dependencies,
        debug_info: None,
    })
}

/// Convert a canonical constant to a runtime value.
///
/// Function references (`Ref`) need group context to resolve internal member
/// indices, so they are handled by the caller ([`to_compiled`]) before this;
/// a stray `Ref` here can only be a final external hash (as in a leaf value
/// object) and maps straight to a [`Value::FunctionRef`].
fn constant_to_value(constant: &ObjectConstant) -> Value {
    match constant {
        ObjectConstant::Bool(b) => Value::Bool(*b),
        ObjectConstant::Number(n) => Value::Number(*n),
        ObjectConstant::String(s) => Value::String(Arc::new(s.clone())),
        ObjectConstant::Binary(b) => Value::binary(b.clone()),
        ObjectConstant::Ref(ObjectRef::External(h)) => Value::FunctionRef(*h),
        ObjectConstant::Ability(id) => Value::AbilityRef(*id),
        ObjectConstant::AbilityMethod {
            ability,
            uuid,
            signature,
            impl_fn,
        } => Value::AbilityMethodRef(Arc::new(crate::value::AbilityMethodRef {
            ability_id: *ability,
            ability_uuid: *uuid,
            signature: *signature,
            impl_fn: match impl_fn {
                Some(ObjectRef::External(h)) => Some(*h),
                // Internal refs are resolved by `to_compiled` before this.
                Some(ObjectRef::Internal(_)) | None => None,
            },
        })),
        ObjectConstant::ValueRef(h) => Value::ObjectRef(*h),
        // A plain `Unit`, or an internal ref — which has no meaning outside a
        // group and cannot occur in a leaf value object — is the inert Unit.
        ObjectConstant::Unit | ObjectConstant::Ref(ObjectRef::Internal(_)) => Value::Unit,
    }
}

/// Build a value object from a runtime `const` value.
///
/// The value's hash derives only from its type tag and bytes — never the
/// const's name — so identical values across modules share one object. A
/// primitive const references nothing, so `resolve` is never consulted.
///
/// # Errors
///
/// Returns an error if the value kind cannot be content-addressed (e.g. a
/// structured value the encoding does not yet support).
pub fn value_object(value: &Value) -> Result<StoredObject, ObjectError> {
    let constant = constant_from_value(value, &|h| ObjectRef::External(*h))?;
    Ok(StoredObject::Value(constant))
}

fn func_has_internal_refs(func: &ObjectFunction) -> bool {
    let is_internal = |r: &ObjectRef| matches!(r, ObjectRef::Internal(_));
    func.dependencies.iter().any(is_internal)
        || func.constants.iter().any(|c| match c {
            ObjectConstant::Ref(r)
            | ObjectConstant::AbilityMethod {
                impl_fn: Some(r), ..
            } => is_internal(r),
            _ => false,
        })
}

fn check_internal_refs(func: &ObjectFunction, member_count: u32) -> Result<(), ObjectError> {
    let check = |r: &ObjectRef| match r {
        ObjectRef::Internal(i) if *i >= member_count => Err(ObjectError::InternalRefOutOfRange {
            index: *i,
            member_count,
        }),
        _ => Ok(()),
    };
    for dep in &func.dependencies {
        check(dep)?;
    }
    for constant in &func.constants {
        match constant {
            ObjectConstant::Ref(r)
            | ObjectConstant::AbilityMethod {
                impl_fn: Some(r), ..
            } => check(r)?,
            _ => {}
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Low-level encoding
// ─────────────────────────────────────────────────────────────────────────────

fn encode_function(out: &mut Vec<u8>, func: &ObjectFunction) {
    out.extend_from_slice(&(func.bytecode.len() as u32).to_le_bytes());
    out.extend_from_slice(&func.bytecode);
    out.extend_from_slice(&func.local_count.to_le_bytes());
    out.push(func.param_count);
    out.extend_from_slice(&(func.constants.len() as u32).to_le_bytes());
    for constant in &func.constants {
        encode_constant(out, constant);
    }
    out.extend_from_slice(&(func.dependencies.len() as u32).to_le_bytes());
    for dep in &func.dependencies {
        encode_ref(out, dep);
    }
}

fn encode_constant(out: &mut Vec<u8>, constant: &ObjectConstant) {
    match constant {
        ObjectConstant::Unit => out.push(CONST_UNIT),
        ObjectConstant::Bool(b) => {
            out.push(CONST_BOOL);
            out.push(u8::from(*b));
        }
        ObjectConstant::Number(n) => {
            out.push(CONST_NUMBER);
            out.extend_from_slice(&n.to_bits().to_le_bytes());
        }
        ObjectConstant::String(s) => {
            out.push(CONST_STRING);
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        ObjectConstant::Binary(b) => {
            out.push(CONST_BYTES);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        ObjectConstant::Ref(r) => {
            out.push(CONST_REF);
            encode_ref(out, r);
        }
        ObjectConstant::Ability(id) => {
            out.push(CONST_ABILITY);
            out.extend_from_slice(id.as_bytes());
        }
        ObjectConstant::ValueRef(h) => {
            out.push(CONST_VALUEREF);
            out.extend_from_slice(h.as_bytes());
        }
        ObjectConstant::AbilityMethod {
            ability,
            uuid,
            signature,
            impl_fn,
        } => {
            out.push(CONST_ABILITY_METHOD);
            out.extend_from_slice(ability.as_bytes());
            out.extend_from_slice(uuid.as_bytes());
            out.extend_from_slice(signature.as_bytes());
            match impl_fn {
                Some(r) => {
                    out.push(1);
                    encode_ref(out, r);
                }
                None => out.push(0),
            }
        }
    }
}

fn encode_ref(out: &mut Vec<u8>, r: &ObjectRef) {
    match r {
        ObjectRef::External(h) => {
            out.push(REF_EXTERNAL);
            out.extend_from_slice(h.as_bytes());
        }
        ObjectRef::Internal(i) => {
            out.push(REF_INTERNAL);
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ObjectError> {
        let end = self.pos.checked_add(n).ok_or(ObjectError::Truncated)?;
        if end > self.bytes.len() {
            return Err(ObjectError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ObjectError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ObjectError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, ObjectError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn decode_function(r: &mut Reader<'_>) -> Result<ObjectFunction, ObjectError> {
    let bytecode_len = r.u32()? as usize;
    let bytecode = r.take(bytecode_len)?.to_vec();
    let local_count = r.u16()?;
    let param_count = r.u8()?;

    let const_count = r.u32()?;
    let mut constants = Vec::with_capacity((const_count as usize).min(r.remaining()));
    for _ in 0..const_count {
        constants.push(decode_constant(r)?);
    }

    let dep_count = r.u32()?;
    let mut dependencies = Vec::with_capacity((dep_count as usize).min(r.remaining()));
    for _ in 0..dep_count {
        dependencies.push(decode_ref(r)?);
    }

    Ok(ObjectFunction {
        bytecode,
        constants,
        local_count,
        param_count,
        dependencies,
    })
}

fn decode_constant(r: &mut Reader<'_>) -> Result<ObjectConstant, ObjectError> {
    Ok(match r.u8()? {
        CONST_UNIT => ObjectConstant::Unit,
        CONST_BOOL => ObjectConstant::Bool(r.u8()? != 0),
        CONST_NUMBER => {
            let b = r.take(8)?;
            let bits = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            ObjectConstant::Number(f64::from_bits(bits))
        }
        CONST_STRING => {
            let len = r.u32()? as usize;
            let raw = r.take(len)?;
            ObjectConstant::String(
                std::str::from_utf8(raw)
                    .map_err(|_| ObjectError::InvalidUtf8)?
                    .to_string(),
            )
        }
        CONST_BYTES => {
            let len = r.u32()? as usize;
            ObjectConstant::Binary(r.take(len)?.to_vec())
        }
        CONST_REF => ObjectConstant::Ref(decode_ref(r)?),
        CONST_ABILITY => {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(r.take(32)?);
            ObjectConstant::Ability(ambient_core::AbilityId::from_bytes(bytes))
        }
        CONST_VALUEREF => {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(r.take(32)?);
            ObjectConstant::ValueRef(blake3::Hash::from_bytes(bytes))
        }
        CONST_ABILITY_METHOD => {
            let mut ability = [0u8; 32];
            ability.copy_from_slice(r.take(32)?);
            let mut uuid_bytes = [0u8; 16];
            uuid_bytes.copy_from_slice(r.take(16)?);
            let mut sig = [0u8; 32];
            sig.copy_from_slice(r.take(32)?);
            let impl_fn = match r.u8()? {
                0 => None,
                1 => Some(decode_ref(r)?),
                t => return Err(ObjectError::BadTag(t)),
            };
            ObjectConstant::AbilityMethod {
                ability: ambient_core::AbilityId::from_bytes(ability),
                uuid: uuid::Uuid::from_bytes(uuid_bytes),
                signature: ambient_core::SignatureHash::from_bytes(sig),
                impl_fn,
            }
        }
        t => return Err(ObjectError::BadTag(t)),
    })
}

fn decode_ref(r: &mut Reader<'_>) -> Result<ObjectRef, ObjectError> {
    Ok(match r.u8()? {
        REF_EXTERNAL => {
            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(r.take(32)?);
            ObjectRef::External(blake3::Hash::from_bytes(hash_bytes))
        }
        REF_INTERNAL => ObjectRef::Internal(r.u32()?),
        t => return Err(ObjectError::BadTag(t)),
    })
}

#[cfg(test)]
mod tests;
