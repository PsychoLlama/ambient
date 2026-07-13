//! The persisted **pre-link symbolic form** of a compiled module: the exact
//! inputs [`assemble_module`](super::assemble::assemble_module) folds into a
//! finished [`CompiledModule`], captured so a body-only edit to a *dependency*
//! can be absorbed without re-checking or re-compiling the dependent.
//!
//! # Why this exists
//!
//! A dependency's body edit moves its callee's final content hash, so a
//! dependent's compiled objects fail link validation and the build cache
//! recompiles them — even though the check output is provably unchanged (check
//! keys exclude foreign bodies). The relink fast path instead reloads this
//! symbolic form, **remaps the moved foreign hashes** (from the recorded
//! consumed-links against the current linking state), and re-runs
//! `assemble_module`. Because finalization is a deterministic function of these
//! inputs, the result is byte-identical to a cold recompile — with no check and
//! no codegen.
//!
//! # Determinism
//!
//! [`PrelinkModule::encode`] sorts every collection canonically, so the same
//! source always produces the same bytes and thus the same blake3 name. This is
//! what lets a warm build's manifest (which records each module's prelink hash)
//! stay byte-identical to a cold build's. Finalization itself is order-
//! independent, so the sort never perturbs the assembled module.
//!
//! # Encoding
//!
//! ```text
//! magic "ABPL" | version u8
//! functions:  u32 | [ name:str, is_main u8, temp_hash[32], plain-object:blob ]
//! lambdas:    u32 | [ temp_hash[32], parent:str, plain-object:blob ]
//! groups:     u32 | [ symbol:str, uuid_string:str ]
//! consts:     u32 | [ value-object:blob ]
//! natives:    u32 | [ native-object:blob ]
//! constNames: u32 | [ name:str, hash[32] ]
//! nativeNames:u32 | [ name:str, hash[32] ]
//! migrations: u32 | [ cell:str, old:str, new:str ]
//! checks:     u32 | [ symbol:str, uuid[16], sig?(u8,[32]), ability:str, method:str, span(u32,u32) ]
//! signatures: u32 | [ name:str, sig:str ]
//! ```
//!
//! Each `blob` is a length-prefixed canonical [`StoredObject`] encoding: a
//! function is a `Plain` object (all references external — the symbolic form
//! has no group-internal indices), a const a `Value`, a native a `Native`.
//! Reusing the object encoding keeps this channel from drifting from the one
//! authority on how a function's bytes are laid out.

#![allow(clippy::cast_possible_truncation)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::bytecode::CompiledFunction;
use crate::object::{
    ObjectError, ObjectRef, StoredObject, compiled_from_object_function, function_from_compiled,
};
use crate::value::Value;

use super::assemble::{AbilityMethodCheck, AssembleInputs};
use super::module_output::MigrationRecord;

/// Magic bytes identifying a pre-link module encoding.
const PRELINK_MAGIC: [u8; 4] = *b"ABPL";

/// Current pre-link encoding version. A blob at any other version reads back
/// as "no prelink" — the designed cold-start / cache-miss behavior.
const PRELINK_VERSION: u8 = 1;

/// One named/impl/ability function in the symbolic form.
#[derive(Debug, Clone)]
pub struct PrelinkFn {
    /// The function's linking name (or dispatch symbol).
    pub name: Arc<str>,
    /// Whether this is the module entry point (`run`).
    pub is_main: bool,
    /// The function, with its temporary (pre-finalization) hash in `hash` and
    /// references carrying temporary/foreign hashes.
    pub func: CompiledFunction,
}

/// One lambda in the symbolic form.
#[derive(Debug, Clone)]
pub struct PrelinkLambda {
    /// The lambda's counter-derived temporary hash (also in `func.hash`).
    pub temp_hash: blake3::Hash,
    /// The parent function's name.
    pub parent: Arc<str>,
    /// The lambda body, with references carrying temporary/foreign hashes.
    pub func: CompiledFunction,
}

/// A module's complete pre-link symbolic form. Everything
/// [`assemble_module`](super::assemble::assemble_module) needs, plus the bare
/// `signatures` the checker attaches at the compile seam.
#[derive(Debug, Clone)]
pub struct PrelinkModule {
    /// Named/impl/ability functions.
    pub functions: Vec<PrelinkFn>,
    /// Lambda bodies.
    pub lambdas: Vec<PrelinkLambda>,
    /// Ability default-implementation symbol → rename-stable group name.
    pub ability_group_names: Vec<(Arc<str>, Arc<str>)>,
    /// Content-addressed `const` value objects (deduplicated by hash).
    pub const_objects: Vec<StoredObject>,
    /// `extern fn` native objects (deduplicated by hash).
    pub native_objects: Vec<StoredObject>,
    /// `const` name → value-object hash.
    pub const_names: Vec<(Arc<str>, blake3::Hash)>,
    /// `extern fn` name → native-object hash.
    pub native_names: Vec<(Arc<str>, blake3::Hash)>,
    /// Static `State::init_versioned` migration obligations.
    pub migrations: Vec<MigrationRecord>,
    /// Ability-method ambiguity-check inputs.
    pub ability_checks: Vec<AbilityMethodCheck>,
    /// Bare `name → canonical signature` bindings (attached at the compile
    /// seam from the checker, exactly as `CompiledModule::signatures` is).
    pub signatures: Vec<(Arc<str>, Arc<str>)>,
}

/// An error decoding a [`PrelinkModule`]. Every variant is a soft cache miss,
/// never a hard build error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrelinkError {
    /// Input ended before the encoding was complete.
    Truncated,
    /// Input did not start with the prelink magic.
    BadMagic,
    /// Unknown or unsupported prelink version.
    BadVersion(u8),
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// An unexpected discriminant byte.
    BadTag(u8),
    /// An embedded object blob did not decode, or was the wrong kind.
    BadObject(String),
    /// Bytes remained after a complete module was decoded.
    TrailingBytes,
}

impl std::fmt::Display for PrelinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "prelink blob is truncated"),
            Self::BadMagic => write!(f, "not a prelink blob (bad magic)"),
            Self::BadVersion(v) => write!(f, "unsupported prelink version {v}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in prelink blob"),
            Self::BadTag(t) => write!(f, "unexpected tag {t} in prelink blob"),
            Self::BadObject(msg) => write!(f, "bad embedded object in prelink blob: {msg}"),
            Self::TrailingBytes => write!(f, "trailing bytes after prelink blob"),
        }
    }
}

impl std::error::Error for PrelinkError {}

impl PrelinkModule {
    /// Build the symbolic form from the pieces a cold compile produced. Sorts
    /// and deduplicates so the encoding is canonical regardless of the source
    /// order the compiler emitted them in.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // one flat capture of the compile outputs
    pub(crate) fn from_compile(
        functions: Vec<PrelinkFn>,
        lambdas: Vec<PrelinkLambda>,
        ability_group_names: HashMap<Arc<str>, Arc<str>>,
        const_objects: Vec<StoredObject>,
        const_names: HashMap<Arc<str>, blake3::Hash>,
        native_objects: Vec<StoredObject>,
        native_names: HashMap<Arc<str>, blake3::Hash>,
        migrations: Vec<MigrationRecord>,
        ability_checks: Vec<AbilityMethodCheck>,
    ) -> Self {
        let mut functions = functions;
        functions.sort_by(|a, b| a.name.cmp(&b.name));
        let mut lambdas = lambdas;
        lambdas.sort_by(|a, b| a.temp_hash.as_bytes().cmp(b.temp_hash.as_bytes()));
        let mut ability_group_names: Vec<(Arc<str>, Arc<str>)> =
            ability_group_names.into_iter().collect();
        ability_group_names.sort();
        let const_objects = dedup_objects(const_objects);
        let native_objects = dedup_objects(native_objects);
        let mut const_names: Vec<(Arc<str>, blake3::Hash)> = const_names.into_iter().collect();
        const_names.sort_by(|a, b| a.0.cmp(&b.0));
        let mut native_names: Vec<(Arc<str>, blake3::Hash)> = native_names.into_iter().collect();
        native_names.sort_by(|a, b| a.0.cmp(&b.0));
        let mut migrations = migrations;
        migrations.sort_by(|a, b| (&a.cell, &a.old, &a.new).cmp(&(&b.cell, &b.old, &b.new)));
        let mut ability_checks = ability_checks;
        ability_checks.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        Self {
            functions,
            lambdas,
            ability_group_names,
            const_objects,
            native_objects,
            const_names,
            native_names,
            migrations,
            ability_checks,
            signatures: Vec::new(),
        }
    }

    /// Attach the bare checker signatures at the compile seam, sorted for a
    /// canonical encoding.
    pub(crate) fn set_signatures(&mut self, signatures: &HashMap<Arc<str>, Arc<str>>) {
        let mut sigs: Vec<(Arc<str>, Arc<str>)> = signatures
            .iter()
            .map(|(n, s)| (Arc::clone(n), Arc::clone(s)))
            .collect();
        sigs.sort_by(|a, b| a.0.cmp(&b.0));
        self.signatures = sigs;
    }

    /// Clone the finalization inputs into an owned [`AssembleInputs`]. Cloning
    /// is required because `assemble_module` consumes its inputs while this
    /// blob must survive to be persisted (or re-persisted on a relink).
    #[must_use]
    pub(crate) fn to_assemble_inputs(&self) -> AssembleInputs {
        AssembleInputs {
            compiled_functions: self
                .functions
                .iter()
                .map(|f| (Arc::clone(&f.name), f.func.clone(), f.is_main))
                .collect(),
            lambdas: self
                .lambdas
                .iter()
                .map(|l| (l.temp_hash, Arc::clone(&l.parent), l.func.clone()))
                .collect(),
            ability_impl_group_names: self.ability_group_names.iter().cloned().collect(),
            const_objects: self.const_objects.clone(),
            const_names: self.const_names.clone(),
            native_objects: self.native_objects.clone(),
            native_names: self.native_names.clone(),
            migrations: self.migrations.clone(),
            ability_checks: self.ability_checks.clone(),
        }
    }

    /// The bare signatures, cloned for the compile seam.
    #[must_use]
    pub(crate) fn signature_map(&self) -> HashMap<Arc<str>, Arc<str>> {
        self.signatures.iter().cloned().collect()
    }

    /// Rewrite every moved foreign hash in place: each function and lambda's
    /// constant-pool function/ability-method references and dependency list.
    /// Temporary (local) hashes never appear as a remap key — they live in a
    /// disjoint domain (`__temp_hash__`/lambda-counter derived) and cannot
    /// collide with a real content hash — so only genuine foreign references
    /// move.
    pub(crate) fn remap(&mut self, remap: &HashMap<blake3::Hash, blake3::Hash>) {
        for f in &mut self.functions {
            remap_function(&mut f.func, remap);
        }
        for l in &mut self.lambdas {
            remap_function(&mut l.func, remap);
        }
    }

    /// Encode to the canonical byte representation.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectError`] if a function's constant pool holds a value that
    /// cannot be content-addressed (impossible for a module that finalized, but
    /// checked defensively so a bad blob is never written).
    #[allow(clippy::too_many_lines)] // one linear field-by-field encoder
    pub fn encode(&self) -> Result<Vec<u8>, ObjectError> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(&PRELINK_MAGIC);
        w.buf.push(PRELINK_VERSION);

        w.u32(self.functions.len() as u32);
        for f in &self.functions {
            w.str(&f.name);
            w.buf.push(u8::from(f.is_main));
            w.buf.extend_from_slice(f.func.hash.as_bytes());
            w.blob(&plain_bytes(&f.func)?);
        }
        w.u32(self.lambdas.len() as u32);
        for l in &self.lambdas {
            w.buf.extend_from_slice(l.temp_hash.as_bytes());
            w.str(&l.parent);
            w.blob(&plain_bytes(&l.func)?);
        }
        w.u32(self.ability_group_names.len() as u32);
        for (symbol, uuid) in &self.ability_group_names {
            w.str(symbol);
            w.str(uuid);
        }
        w.u32(self.const_objects.len() as u32);
        for object in &self.const_objects {
            w.blob(&object.encode());
        }
        w.u32(self.native_objects.len() as u32);
        for object in &self.native_objects {
            w.blob(&object.encode());
        }
        w.u32(self.const_names.len() as u32);
        for (name, hash) in &self.const_names {
            w.str(name);
            w.buf.extend_from_slice(hash.as_bytes());
        }
        w.u32(self.native_names.len() as u32);
        for (name, hash) in &self.native_names {
            w.str(name);
            w.buf.extend_from_slice(hash.as_bytes());
        }
        w.u32(self.migrations.len() as u32);
        for m in &self.migrations {
            w.str(&m.cell);
            w.str(&m.old);
            w.str(&m.new);
        }
        w.u32(self.ability_checks.len() as u32);
        for check in &self.ability_checks {
            w.str(&check.symbol);
            w.buf.extend_from_slice(check.uuid.as_bytes());
            match &check.signature {
                Some(sig) => {
                    w.buf.push(1);
                    w.buf.extend_from_slice(sig.as_bytes());
                }
                None => w.buf.push(0),
            }
            w.str(&check.ability_name);
            w.str(&check.method_name);
            w.u32(check.span.0);
            w.u32(check.span.1);
        }
        w.u32(self.signatures.len() as u32);
        for (name, sig) in &self.signatures {
            w.str(name);
            w.str(sig);
        }
        Ok(w.buf)
    }

    /// Decode from the canonical byte representation.
    ///
    /// # Errors
    ///
    /// Returns a [`PrelinkError`] if the bytes are not a complete, well-formed
    /// blob of the current version. Callers treat any error as a cache miss.
    #[allow(clippy::too_many_lines)] // one linear field-by-field decoder
    pub fn decode(bytes: &[u8]) -> Result<Self, PrelinkError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(4)? != PRELINK_MAGIC {
            return Err(PrelinkError::BadMagic);
        }
        let version = r.u8()?;
        if version != PRELINK_VERSION {
            return Err(PrelinkError::BadVersion(version));
        }

        let fn_count = r.u32()?;
        let mut functions = Vec::with_capacity((fn_count as usize).min(r.remaining()));
        for _ in 0..fn_count {
            let name = r.str()?;
            let is_main = r.bool()?;
            let temp = r.hash()?;
            let func = r.plain_function(temp)?;
            functions.push(PrelinkFn {
                name: Arc::from(name),
                is_main,
                func,
            });
        }
        let lambda_count = r.u32()?;
        let mut lambdas = Vec::with_capacity((lambda_count as usize).min(r.remaining()));
        for _ in 0..lambda_count {
            let temp = r.hash()?;
            let parent = r.str()?;
            let func = r.plain_function(temp)?;
            lambdas.push(PrelinkLambda {
                temp_hash: temp,
                parent: Arc::from(parent),
                func,
            });
        }
        let group_count = r.u32()?;
        let mut ability_group_names = Vec::with_capacity((group_count as usize).min(r.remaining()));
        for _ in 0..group_count {
            ability_group_names.push((Arc::from(r.str()?), Arc::from(r.str()?)));
        }
        let const_count = r.u32()?;
        let mut const_objects = Vec::with_capacity((const_count as usize).min(r.remaining()));
        for _ in 0..const_count {
            const_objects.push(r.object()?);
        }
        let native_count = r.u32()?;
        let mut native_objects = Vec::with_capacity((native_count as usize).min(r.remaining()));
        for _ in 0..native_count {
            native_objects.push(r.object()?);
        }
        let const_name_count = r.u32()?;
        let mut const_names = Vec::with_capacity((const_name_count as usize).min(r.remaining()));
        for _ in 0..const_name_count {
            const_names.push((Arc::from(r.str()?), r.hash()?));
        }
        let native_name_count = r.u32()?;
        let mut native_names = Vec::with_capacity((native_name_count as usize).min(r.remaining()));
        for _ in 0..native_name_count {
            native_names.push((Arc::from(r.str()?), r.hash()?));
        }
        let migration_count = r.u32()?;
        let mut migrations = Vec::with_capacity((migration_count as usize).min(r.remaining()));
        for _ in 0..migration_count {
            migrations.push(MigrationRecord {
                cell: Arc::from(r.str()?),
                old: Arc::from(r.str()?),
                new: Arc::from(r.str()?),
            });
        }
        let check_count = r.u32()?;
        let mut ability_checks = Vec::with_capacity((check_count as usize).min(r.remaining()));
        for _ in 0..check_count {
            let symbol = Arc::from(r.str()?);
            let uuid = uuid::Uuid::from_bytes(r.uuid()?);
            let signature = match r.u8()? {
                0 => None,
                1 => Some(ambient_core::SignatureHash::from_bytes(r.hash_bytes()?)),
                other => return Err(PrelinkError::BadTag(other)),
            };
            let ability_name = Arc::from(r.str()?);
            let method_name = Arc::from(r.str()?);
            let span = (r.u32()?, r.u32()?);
            ability_checks.push(AbilityMethodCheck {
                symbol,
                uuid,
                signature,
                ability_name,
                method_name,
                span,
            });
        }
        let sig_count = r.u32()?;
        let mut signatures = Vec::with_capacity((sig_count as usize).min(r.remaining()));
        for _ in 0..sig_count {
            signatures.push((Arc::from(r.str()?), Arc::from(r.str()?)));
        }

        if r.pos != bytes.len() {
            return Err(PrelinkError::TrailingBytes);
        }
        Ok(Self {
            functions,
            lambdas,
            ability_group_names,
            const_objects,
            native_objects,
            const_names,
            native_names,
            migrations,
            ability_checks,
            signatures,
        })
    }
}

/// Deduplicate leaf objects by content hash, sorted by hash for a canonical
/// encoding. Value/native objects are leaves, so identity is their hash.
fn dedup_objects(objects: Vec<StoredObject>) -> Vec<StoredObject> {
    let mut by_hash: BTreeMap<[u8; 32], StoredObject> = BTreeMap::new();
    for object in objects {
        by_hash.entry(*object.hash().as_bytes()).or_insert(object);
    }
    by_hash.into_values().collect()
}

/// A function's canonical `Plain`-object bytes: every reference external (the
/// symbolic form has no group-internal indices).
fn plain_bytes(func: &CompiledFunction) -> Result<Vec<u8>, ObjectError> {
    let object = function_from_compiled(func, &|h| ObjectRef::External(*h))?;
    Ok(StoredObject::Plain(object).encode())
}

/// Rewrite one function's foreign references through `remap`.
fn remap_function(func: &mut CompiledFunction, remap: &HashMap<blake3::Hash, blake3::Hash>) {
    for constant in &mut func.constants {
        match constant {
            Value::FunctionRef(h) => {
                if let Some(new) = remap.get(h) {
                    *h = *new;
                }
            }
            Value::AbilityMethodRef(m) => {
                if let Some(impl_h) = m.impl_fn
                    && let Some(new) = remap.get(&impl_h)
                {
                    let mut updated = (**m).clone();
                    updated.impl_fn = Some(*new);
                    *m = Arc::new(updated);
                }
            }
            _ => {}
        }
    }
    for dep in &mut func.dependencies {
        if let Some(new) = remap.get(dep) {
            *dep = *new;
        }
    }
    // Keep the derived key cache consistent with the rewritten constants;
    // finalization rebuilds it again from the final constants, but a
    // self-consistent function is cheap and avoids surprising a reader.
    func.method_keys = CompiledFunction::index_method_keys(&func.constants);
}

// ─────────────────────────────────────────────────────────────────────────────
// Low-level writer / reader (mirrors the object/manifest encoding discipline)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn u32(&mut self, n: u32) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn blob(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], PrelinkError> {
        let end = self.pos.checked_add(n).ok_or(PrelinkError::Truncated)?;
        if end > self.bytes.len() {
            return Err(PrelinkError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, PrelinkError> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, PrelinkError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(PrelinkError::BadTag(other)),
        }
    }

    fn u32(&mut self) -> Result<u32, PrelinkError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn str(&mut self) -> Result<String, PrelinkError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        std::str::from_utf8(raw)
            .map(ToString::to_string)
            .map_err(|_| PrelinkError::InvalidUtf8)
    }

    fn hash_bytes(&mut self) -> Result<[u8; 32], PrelinkError> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }

    fn hash(&mut self) -> Result<blake3::Hash, PrelinkError> {
        Ok(blake3::Hash::from_bytes(self.hash_bytes()?))
    }

    fn uuid(&mut self) -> Result<[u8; 16], PrelinkError> {
        let mut u = [0u8; 16];
        u.copy_from_slice(self.take(16)?);
        Ok(u)
    }

    /// Read a length-prefixed embedded [`StoredObject`] blob.
    fn object(&mut self) -> Result<StoredObject, PrelinkError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        StoredObject::decode(raw).map_err(|e| PrelinkError::BadObject(e.to_string()))
    }

    /// Read a length-prefixed `Plain` object and reconstruct the function it
    /// carries, tagging it with its temporary `hash`.
    fn plain_function(&mut self, hash: blake3::Hash) -> Result<CompiledFunction, PrelinkError> {
        match self.object()? {
            StoredObject::Plain(func) => compiled_from_object_function(&func, hash)
                .map_err(|e| PrelinkError::BadObject(e.to_string())),
            _ => Err(PrelinkError::BadObject("expected a plain function".into())),
        }
    }
}

#[cfg(test)]
mod tests;
