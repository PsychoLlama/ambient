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
//! ```text
//! header:   magic "ABOB" | version u8 = 1 | kind u8 (0 plain, 1 group, 2 redirect)
//! plain:    body
//! group:    member_count u32 | members: [has_name u8, (name_len u32, name)?, body]
//! redirect: group_hash [32] | index u32
//! body:     bytecode_len u32 | bytecode
//!           local_count u16 | param_count u8
//!           const_count u32 | constants
//!           dep_count u32 | refs
//! ref:      0 u8 | hash [32]        (external)
//!           1 u8 | index u32        (internal to the group)
//! constant: 0 unit | 1 bool u8 | 2 number f64-bits | 3 string (u32, utf8)
//!           4 bytes (u32, raw) | 5 ref
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
pub const OBJECT_VERSION: u8 = 3;

const KIND_PLAIN: u8 = 0;
const KIND_GROUP: u8 = 1;
const KIND_REDIRECT: u8 = 2;

const REF_EXTERNAL: u8 = 0;
const REF_INTERNAL: u8 = 1;

const CONST_UNIT: u8 = 0;
const CONST_BOOL: u8 = 1;
const CONST_NUMBER: u8 = 2;
const CONST_STRING: u8 = 3;
const CONST_BYTES: u8 = 4;
const CONST_REF: u8 = 5;
const CONST_ABILITY: u8 = 6;

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
    Bytes(Vec<u8>),
    Ref(ObjectRef),
    /// The content-addressed identity of an ability interface.
    Ability(ambient_core::AbilityId),
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
    /// Bytes remained after a complete object was decoded.
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

    /// Turn this object into runnable functions with final hashes.
    ///
    /// Returns `(hash, function)` pairs: one for a plain object, one per
    /// member for a group. All internal references are substituted with the
    /// derived member hashes.
    ///
    /// # Errors
    ///
    /// Returns an error for redirects (they carry no code) or malformed
    /// internal references.
    pub fn materialize(&self) -> Result<Vec<(blake3::Hash, CompiledFunction)>, ObjectError> {
        match self {
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
        Value::Bytes(b) => ObjectConstant::Bytes((**b).clone()),
        Value::FunctionRef(h) => ObjectConstant::Ref(resolve(h)),
        Value::AbilityRef(id) => ObjectConstant::Ability(*id),
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
                ObjectConstant::Unit => Value::Unit,
                ObjectConstant::Bool(b) => Value::Bool(*b),
                ObjectConstant::Number(n) => Value::Number(*n),
                ObjectConstant::String(s) => Value::String(Arc::new(s.clone())),
                ObjectConstant::Bytes(b) => Value::bytes(b.clone()),
                ObjectConstant::Ref(r) => Value::FunctionRef(resolve(r)?),
                ObjectConstant::Ability(id) => Value::AbilityRef(*id),
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

fn func_has_internal_refs(func: &ObjectFunction) -> bool {
    let is_internal = |r: &ObjectRef| matches!(r, ObjectRef::Internal(_));
    func.dependencies.iter().any(is_internal)
        || func.constants.iter().any(|c| match c {
            ObjectConstant::Ref(r) => is_internal(r),
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
        if let ObjectConstant::Ref(r) = constant {
            check(r)?;
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
        ObjectConstant::Bytes(b) => {
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
            ObjectConstant::Bytes(r.take(len)?.to_vec())
        }
        CONST_REF => ObjectConstant::Ref(decode_ref(r)?),
        CONST_ABILITY => {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(r.take(32)?);
            ObjectConstant::Ability(ambient_core::AbilityId::from_bytes(bytes))
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
mod tests {
    use super::*;

    fn sample_function() -> ObjectFunction {
        ObjectFunction {
            bytecode: vec![1, 2, 3, 4],
            constants: vec![
                ObjectConstant::Unit,
                ObjectConstant::Bool(true),
                ObjectConstant::Number(42.5),
                ObjectConstant::String("hello".to_string()),
                ObjectConstant::Bytes(vec![0xde, 0xad]),
                ObjectConstant::Ref(ObjectRef::External(blake3::hash(b"dep"))),
                ObjectConstant::Ability(ambient_core::AbilityId::from_bytes([0xab; 32])),
            ],
            local_count: 3,
            param_count: 2,
            dependencies: vec![ObjectRef::External(blake3::hash(b"dep"))],
        }
    }

    #[test]
    fn plain_roundtrip() {
        let object = StoredObject::Plain(sample_function());
        let encoded = object.encode();
        let decoded = StoredObject::decode(&encoded).unwrap();
        assert_eq!(object, decoded);
        assert_eq!(object.hash(), decoded.hash());
    }

    #[test]
    fn group_roundtrip() {
        let mut even = sample_function();
        even.constants
            .push(ObjectConstant::Ref(ObjectRef::Internal(1)));
        even.dependencies.push(ObjectRef::Internal(1));
        let mut odd = sample_function();
        odd.constants
            .push(ObjectConstant::Ref(ObjectRef::Internal(0)));
        odd.dependencies.push(ObjectRef::Internal(0));

        let object = StoredObject::Group(vec![
            GroupMember {
                name: Some("even".to_string()),
                function: even,
            },
            GroupMember {
                name: Some("odd".to_string()),
                function: odd,
            },
        ]);
        let encoded = object.encode();
        let decoded = StoredObject::decode(&encoded).unwrap();
        assert_eq!(object, decoded);
    }

    #[test]
    fn redirect_roundtrip() {
        let object = StoredObject::Redirect {
            group: blake3::hash(b"group"),
            index: 7,
        };
        let decoded = StoredObject::decode(&object.encode()).unwrap();
        assert_eq!(object, decoded);
    }

    #[test]
    fn trailing_bytes_rejected() {
        let mut encoded = StoredObject::Plain(sample_function()).encode();
        encoded.push(0);
        assert_eq!(
            StoredObject::decode(&encoded),
            Err(ObjectError::TrailingBytes)
        );
    }

    #[test]
    fn truncation_rejected() {
        let encoded = StoredObject::Plain(sample_function()).encode();
        for len in 0..encoded.len() {
            assert!(
                StoredObject::decode(&encoded[..len]).is_err(),
                "prefix of length {len} should not decode"
            );
        }
    }

    #[test]
    fn lying_length_prefix_fails_without_huge_allocation() {
        // A corrupted count must produce a decode error, not an attempted
        // multi-gigabyte allocation. The encoding ends with the dependency
        // list: count u32 | (tag u8 | hash [32]). Corrupt the count's high
        // byte.
        let mut encoded = StoredObject::Plain(sample_function()).encode();
        let count_high_byte = encoded.len() - 33 - 1;
        encoded[count_high_byte] = 0xff;
        assert!(StoredObject::decode(&encoded).is_err());
    }

    #[test]
    fn corruption_changes_hash() {
        let object = StoredObject::Plain(sample_function());
        let mut encoded = object.encode();
        let hash = blake3::hash(&encoded);
        // Flip a bytecode byte: still decodes, but hash must differ.
        let idx = encoded.len() - 40;
        encoded[idx] ^= 0xff;
        assert_ne!(blake3::hash(&encoded), hash);
    }

    #[test]
    fn internal_ref_in_plain_rejected() {
        let mut func = sample_function();
        func.dependencies.push(ObjectRef::Internal(0));
        let encoded = StoredObject::Plain(func).encode();
        assert_eq!(
            StoredObject::decode(&encoded),
            Err(ObjectError::InternalRefInPlain)
        );
    }

    #[test]
    fn internal_ref_out_of_range_rejected() {
        let mut func = sample_function();
        func.dependencies.push(ObjectRef::Internal(5));
        let encoded = StoredObject::Group(vec![GroupMember {
            name: Some("f".to_string()),
            function: func,
        }])
        .encode();
        assert!(matches!(
            StoredObject::decode(&encoded),
            Err(ObjectError::InternalRefOutOfRange { .. })
        ));
    }

    #[test]
    fn plain_materialize_hash_is_self_verifying() {
        let object = StoredObject::Plain(sample_function());
        let materialized = object.materialize().unwrap();
        assert_eq!(materialized.len(), 1);
        let (hash, func) = &materialized[0];
        assert_eq!(*hash, blake3::hash(&object.encode()));
        assert_eq!(func.hash, *hash);
    }

    #[test]
    fn group_materialize_substitutes_member_hashes() {
        let mut a = sample_function();
        a.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(1))];
        a.dependencies = vec![ObjectRef::Internal(1)];
        let mut b = sample_function();
        b.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(0))];
        b.dependencies = vec![ObjectRef::Internal(0)];

        let object = StoredObject::Group(vec![
            GroupMember {
                name: Some("a".to_string()),
                function: a,
            },
            GroupMember {
                name: Some("b".to_string()),
                function: b,
            },
        ]);
        let group = object.hash();
        let materialized = object.materialize().unwrap();
        assert_eq!(materialized.len(), 2);

        let hash_a = member_hash(&group, 0, 2);
        let hash_b = member_hash(&group, 1, 2);
        assert_eq!(materialized[0].0, hash_a);
        assert_eq!(materialized[1].0, hash_b);
        // a's references point at b and vice versa.
        assert_eq!(materialized[0].1.dependencies, vec![hash_b]);
        assert_eq!(materialized[1].1.dependencies, vec![hash_a]);
        assert!(matches!(
            materialized[0].1.constants[0],
            Value::FunctionRef(h) if h == hash_b
        ));
    }

    #[test]
    fn singleton_group_member_hash_is_group_hash() {
        let mut f = sample_function();
        f.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(0))];
        f.dependencies = vec![ObjectRef::Internal(0)];
        let object = StoredObject::Group(vec![GroupMember {
            name: Some("loop_forever".to_string()),
            function: f,
        }]);
        let group = object.hash();
        let materialized = object.materialize().unwrap();
        assert_eq!(materialized[0].0, group);
        // Self-reference resolves to its own hash.
        assert_eq!(materialized[0].1.dependencies, vec![group]);
    }

    #[test]
    fn member_name_affects_group_hash() {
        let make = |name: &str| {
            StoredObject::Group(vec![GroupMember {
                name: Some(name.to_string()),
                function: sample_function(),
            }])
        };
        assert_ne!(make("a").hash(), make("b").hash());
    }

    #[test]
    fn nan_number_roundtrips_exactly() {
        let bits = 0x7ff8_dead_beef_0001_u64;
        let func = ObjectFunction {
            bytecode: vec![],
            constants: vec![ObjectConstant::Number(f64::from_bits(bits))],
            local_count: 0,
            param_count: 0,
            dependencies: vec![],
        };
        let object = StoredObject::Plain(func);
        let decoded = StoredObject::decode(&object.encode()).unwrap();
        let StoredObject::Plain(f) = decoded else {
            panic!("expected plain")
        };
        let ObjectConstant::Number(n) = f.constants[0] else {
            panic!("expected number")
        };
        assert_eq!(n.to_bits(), bits);
    }
}
