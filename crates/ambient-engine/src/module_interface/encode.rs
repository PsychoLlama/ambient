//! Canonical byte encoding for [`ModuleInterface`], following the
//! `object` module's discipline: magic + version byte + little-endian
//! length-prefixed fields, and a total decode that rejects trailing bytes
//! and unknown tags. `decode ∘ encode` is the identity, so every byte of an
//! interface is covered by [`ModuleInterface::interface_hash`].

#![allow(clippy::cast_possible_truncation)]

use super::{
    AbilityMethodEntry, AbilityShape, AliasShape, ConstEntry, EnumShape, ExternEntry, FnSig,
    ImplMethodEntry, ImplShape, ModuleInterface, ReExportEntry, StructShape, TraitMethodSig,
    TraitShape,
};

/// Magic bytes identifying an Ambient module-interface encoding.
const INTERFACE_MAGIC: [u8; 4] = *b"ABMI";
/// Current interface encoding version.
const INTERFACE_VERSION: u8 = 1;

/// An error decoding a [`ModuleInterface`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceError {
    /// Input ended before the encoding was complete.
    Truncated,
    /// Input did not start with the interface magic.
    BadMagic,
    /// Unknown encoding version.
    BadVersion(u8),
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// An unknown discriminant tag (e.g. an optional's flag byte).
    BadTag(u8),
    /// Bytes remained after a complete interface was decoded.
    TrailingBytes,
}

impl std::fmt::Display for InterfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "module-interface encoding is truncated"),
            Self::BadMagic => write!(f, "not a module-interface encoding (bad magic)"),
            Self::BadVersion(v) => write!(f, "unsupported module-interface version {v}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in module-interface encoding"),
            Self::BadTag(t) => write!(f, "unknown tag {t} in module-interface encoding"),
            Self::TrailingBytes => write!(f, "trailing bytes after module-interface encoding"),
        }
    }
}

impl std::error::Error for InterfaceError {}

impl ModuleInterface {
    /// Encode to the canonical byte representation.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(&INTERFACE_MAGIC);
        w.buf.push(INTERFACE_VERSION);
        w.str(&self.module);
        w.vec(&self.functions, write_fn_sig);
        w.vec(&self.consts, write_const);
        w.vec(&self.structs, write_struct);
        w.vec(&self.enums, write_enum);
        w.vec(&self.aliases, write_alias);
        w.vec(&self.traits, write_trait);
        w.vec(&self.abilities, write_ability);
        w.vec(&self.impls, write_impl);
        w.vec(&self.reexports, write_reexport);
        w.vec(&self.externs, write_extern);
        w.buf
    }

    /// The impl + ability sections only — the build-global dispatch surface
    /// a package fold consumes.
    pub(crate) fn dispatch_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.vec(&self.impls, write_impl);
        w.vec(&self.abilities, write_ability);
        w.buf
    }

    /// Decode from the canonical byte representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a complete, well-formed
    /// interface (bad magic/version, truncation, invalid UTF-8, unknown
    /// tag, or trailing bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, InterfaceError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(4)? != INTERFACE_MAGIC {
            return Err(InterfaceError::BadMagic);
        }
        let version = r.u8()?;
        if version != INTERFACE_VERSION {
            return Err(InterfaceError::BadVersion(version));
        }
        let out = Self {
            module: r.str()?,
            functions: r.vec(read_fn_sig)?,
            consts: r.vec(read_const)?,
            structs: r.vec(read_struct)?,
            enums: r.vec(read_enum)?,
            aliases: r.vec(read_alias)?,
            traits: r.vec(read_trait)?,
            abilities: r.vec(read_ability)?,
            impls: r.vec(read_impl)?,
            reexports: r.vec(read_reexport)?,
            externs: r.vec(read_extern)?,
        };
        if r.pos != bytes.len() {
            return Err(InterfaceError::TrailingBytes);
        }
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-struct writers / readers
// ─────────────────────────────────────────────────────────────────────────────

fn write_fn_sig(w: &mut Writer, s: &FnSig) {
    w.str(&s.name);
    w.strs(&s.params);
    w.str(&s.ret);
    w.strs(&s.abilities);
}

fn read_fn_sig(r: &mut Reader<'_>) -> Result<FnSig, InterfaceError> {
    Ok(FnSig {
        name: r.str()?,
        params: r.strs()?,
        ret: r.str()?,
        abilities: r.strs()?,
    })
}

fn write_const(w: &mut Writer, c: &ConstEntry) {
    w.str(&c.name);
    w.str(&c.ty);
    w.opt_hash(c.value_hash.as_ref());
}

fn read_const(r: &mut Reader<'_>) -> Result<ConstEntry, InterfaceError> {
    Ok(ConstEntry {
        name: r.str()?,
        ty: r.str()?,
        value_hash: r.opt_hash()?,
    })
}

fn write_struct(w: &mut Writer, s: &StructShape) {
    w.str(&s.name);
    w.str(&s.uuid);
    w.bool(s.is_extern);
    w.strs(&s.type_params);
    w.u32(s.fields.len() as u32);
    for (name, ty) in &s.fields {
        w.str(name);
        w.str(ty);
    }
}

fn read_struct(r: &mut Reader<'_>) -> Result<StructShape, InterfaceError> {
    let name = r.str()?;
    let uuid = r.str()?;
    let is_extern = r.bool()?;
    let type_params = r.strs()?;
    let count = r.u32()?;
    let mut fields = Vec::with_capacity((count as usize).min(r.remaining()));
    for _ in 0..count {
        fields.push((r.str()?, r.str()?));
    }
    Ok(StructShape {
        name,
        uuid,
        is_extern,
        type_params,
        fields,
    })
}

fn write_enum(w: &mut Writer, e: &EnumShape) {
    w.str(&e.name);
    w.str(&e.uuid);
    w.strs(&e.type_params);
    w.u32(e.variants.len() as u32);
    for (name, payload) in &e.variants {
        w.str(name);
        w.opt_str(payload.as_deref());
    }
}

fn read_enum(r: &mut Reader<'_>) -> Result<EnumShape, InterfaceError> {
    let name = r.str()?;
    let uuid = r.str()?;
    let type_params = r.strs()?;
    let count = r.u32()?;
    let mut variants = Vec::with_capacity((count as usize).min(r.remaining()));
    for _ in 0..count {
        variants.push((r.str()?, r.opt_str()?));
    }
    Ok(EnumShape {
        name,
        uuid,
        type_params,
        variants,
    })
}

fn write_alias(w: &mut Writer, a: &AliasShape) {
    w.str(&a.name);
    w.strs(&a.type_params);
    w.str(&a.target);
}

fn read_alias(r: &mut Reader<'_>) -> Result<AliasShape, InterfaceError> {
    Ok(AliasShape {
        name: r.str()?,
        type_params: r.strs()?,
        target: r.str()?,
    })
}

fn write_trait(w: &mut Writer, t: &TraitShape) {
    w.str(&t.name);
    w.str(&t.uuid);
    w.strs(&t.type_params);
    w.strs(&t.supertraits);
    w.vec(&t.methods, write_trait_method);
}

fn read_trait(r: &mut Reader<'_>) -> Result<TraitShape, InterfaceError> {
    Ok(TraitShape {
        name: r.str()?,
        uuid: r.str()?,
        type_params: r.strs()?,
        supertraits: r.strs()?,
        methods: r.vec(read_trait_method)?,
    })
}

fn write_trait_method(w: &mut Writer, m: &TraitMethodSig) {
    w.str(&m.name);
    w.bool(m.has_self);
    w.strs(&m.params);
    w.str(&m.ret);
    w.strs(&m.abilities);
}

fn read_trait_method(r: &mut Reader<'_>) -> Result<TraitMethodSig, InterfaceError> {
    Ok(TraitMethodSig {
        name: r.str()?,
        has_self: r.bool()?,
        params: r.strs()?,
        ret: r.str()?,
        abilities: r.strs()?,
    })
}

fn write_ability(w: &mut Writer, a: &AbilityShape) {
    w.str(&a.name);
    w.buf.extend_from_slice(&a.ability_id);
    w.strs(&a.dependencies);
    w.vec(&a.methods, write_ability_method);
}

fn read_ability(r: &mut Reader<'_>) -> Result<AbilityShape, InterfaceError> {
    Ok(AbilityShape {
        name: r.str()?,
        ability_id: r.hash()?,
        dependencies: r.strs()?,
        methods: r.vec(read_ability_method)?,
    })
}

fn write_ability_method(w: &mut Writer, m: &AbilityMethodEntry) {
    w.str(&m.name);
    w.strs(&m.params);
    w.str(&m.ret);
    w.bool(m.never);
    w.opt_hash(m.body_hash.as_ref());
}

fn read_ability_method(r: &mut Reader<'_>) -> Result<AbilityMethodEntry, InterfaceError> {
    Ok(AbilityMethodEntry {
        name: r.str()?,
        params: r.strs()?,
        ret: r.str()?,
        never: r.bool()?,
        body_hash: r.opt_hash()?,
    })
}

fn write_impl(w: &mut Writer, i: &ImplShape) {
    w.opt_str(i.trait_ref.as_deref());
    w.str(&i.for_type);
    w.strs(&i.type_params);
    w.vec(&i.methods, write_impl_method);
}

fn read_impl(r: &mut Reader<'_>) -> Result<ImplShape, InterfaceError> {
    Ok(ImplShape {
        trait_ref: r.opt_str()?,
        for_type: r.str()?,
        type_params: r.strs()?,
        methods: r.vec(read_impl_method)?,
    })
}

fn write_impl_method(w: &mut Writer, m: &ImplMethodEntry) {
    w.str(&m.name);
    w.bool(m.has_self);
    w.strs(&m.params);
    w.str(&m.ret);
    w.strs(&m.abilities);
    w.buf.extend_from_slice(&m.body_hash);
}

fn read_impl_method(r: &mut Reader<'_>) -> Result<ImplMethodEntry, InterfaceError> {
    Ok(ImplMethodEntry {
        name: r.str()?,
        has_self: r.bool()?,
        params: r.strs()?,
        ret: r.str()?,
        abilities: r.strs()?,
        body_hash: r.hash()?,
    })
}

fn write_reexport(w: &mut Writer, re: &ReExportEntry) {
    w.str(&re.local);
    w.buf.push(re.kind);
    w.str(&re.target);
}

fn read_reexport(r: &mut Reader<'_>) -> Result<ReExportEntry, InterfaceError> {
    Ok(ReExportEntry {
        local: r.str()?,
        kind: r.u8()?,
        target: r.str()?,
    })
}

fn write_extern(w: &mut Writer, e: &ExternEntry) {
    w.str(&e.name);
    w.opt_str(e.uuid.as_deref());
    w.buf.push(e.arity);
}

fn read_extern(r: &mut Reader<'_>) -> Result<ExternEntry, InterfaceError> {
    Ok(ExternEntry {
        name: r.str()?,
        uuid: r.opt_str()?,
        arity: r.u8()?,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Low-level writer / reader
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn u32(&mut self, n: u32) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    fn bool(&mut self, b: bool) {
        self.buf.push(u8::from(b));
    }

    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn opt_str(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.buf.push(1);
                self.str(s);
            }
            None => self.buf.push(0),
        }
    }

    fn strs(&mut self, items: &[String]) {
        self.u32(items.len() as u32);
        for s in items {
            self.str(s);
        }
    }

    fn opt_hash(&mut self, h: Option<&[u8; 32]>) {
        match h {
            Some(h) => {
                self.buf.push(1);
                self.buf.extend_from_slice(h);
            }
            None => self.buf.push(0),
        }
    }

    fn vec<T>(&mut self, items: &[T], write: impl Fn(&mut Self, &T)) {
        self.u32(items.len() as u32);
        for item in items {
            write(self, item);
        }
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

    fn take(&mut self, n: usize) -> Result<&[u8], InterfaceError> {
        let end = self.pos.checked_add(n).ok_or(InterfaceError::Truncated)?;
        if end > self.bytes.len() {
            return Err(InterfaceError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, InterfaceError> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, InterfaceError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            t => Err(InterfaceError::BadTag(t)),
        }
    }

    fn u32(&mut self) -> Result<u32, InterfaceError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn str(&mut self) -> Result<String, InterfaceError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        std::str::from_utf8(raw)
            .map(ToString::to_string)
            .map_err(|_| InterfaceError::InvalidUtf8)
    }

    fn opt_str(&mut self) -> Result<Option<String>, InterfaceError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.str()?)),
            t => Err(InterfaceError::BadTag(t)),
        }
    }

    fn strs(&mut self) -> Result<Vec<String>, InterfaceError> {
        let count = self.u32()?;
        let mut out = Vec::with_capacity((count as usize).min(self.remaining()));
        for _ in 0..count {
            out.push(self.str()?);
        }
        Ok(out)
    }

    fn hash(&mut self) -> Result<[u8; 32], InterfaceError> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }

    fn opt_hash(&mut self) -> Result<Option<[u8; 32]>, InterfaceError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.hash()?)),
            t => Err(InterfaceError::BadTag(t)),
        }
    }

    fn vec<T>(
        &mut self,
        read: impl Fn(&mut Self) -> Result<T, InterfaceError>,
    ) -> Result<Vec<T>, InterfaceError> {
        let count = self.u32()?;
        let mut out = Vec::with_capacity((count as usize).min(self.remaining()));
        for _ in 0..count {
            out.push(read(self)?);
        }
        Ok(out)
    }
}
