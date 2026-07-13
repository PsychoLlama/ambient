//! Build snapshots: a crash-safe, content-addressed record of one
//! successful build, layered on top of the object store.
//!
//! ```text
//! <root>/
//!   snapshot                  root pointer: "ambient-snapshot-v1 <64hex>\n"
//!   meta/<2hex>/<62hex>       one canonical build manifest per file
//! ```
//!
//! A **manifest** ([`BuildManifest`]) is a versioned, canonical encoding of
//! everything Phase 3 needs to decide "can I skip this module": per module
//! its resolved-AST hash, interface hash, resolve-pass dependency set,
//! produced object hashes, name bindings, and canonical signatures; and per
//! package the name, dispatch-surface hash, native-contract hash, and format
//! version. Manifest bytes are `blake3`-named and self-verifying, exactly
//! like objects — they just live under `meta/` so the runtime object format
//! never has to know about them.
//!
//! The **root pointer** (`snapshot`) names the current manifest. Writing a
//! snapshot is ordered for crash safety: every referenced object is already
//! durable (`put_module` ran first), then the manifest bytes are written and
//! renamed into place, and only *after that* is the pointer atomically
//! swapped. A reader that follows a pointer therefore always finds its
//! manifest, and a manifest always finds its objects — a crash between steps
//! leaves an older-but-consistent snapshot (or none), never a torn one.
//!
//! Every failure mode on the read side — missing pointer, malformed pointer,
//! unknown version, truncated or hash-mismatched manifest — degrades to "no
//! snapshot" for the build path ([`DiskStore::current_snapshot`]), because
//! the store is a rebuildable cache and must never block a build. The same
//! conditions are reported *loudly* by [`DiskStore::verify`].

#![allow(clippy::cast_possible_truncation)]

use std::io;
use std::path::PathBuf;

use super::{DiskStore, DiskStoreError, write_atomic};
use crate::build::BuildResult;

/// Magic bytes identifying an Ambient build-manifest encoding.
const MANIFEST_MAGIC: [u8; 4] = *b"ABSM";

/// Current build-manifest format version. Bumped only on an incompatible
/// encoding change; a manifest at any other version reads back as "no
/// snapshot" (and is reported by `verify`).
pub const MANIFEST_VERSION: u8 = 1;

/// The root-pointer file name under the store root.
pub const SNAPSHOT_POINTER: &str = "snapshot";

/// Prefix (and version marker) of the root-pointer file's single line.
const POINTER_PREFIX: &str = "ambient-snapshot-v1 ";

/// A canonical, versioned record of one successful build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildManifest {
    /// Manifest format version (see [`MANIFEST_VERSION`]).
    pub version: u8,
    /// The package name this build produced.
    pub package_name: String,
    /// The build-global dispatch-surface hash.
    pub dispatch_surface_hash: [u8; 32],
    /// A deterministic hash of the native-binding surface the build saw.
    pub natives_contract_hash: [u8; 32],
    /// Per-module records, sorted by canonical module identity.
    pub modules: Vec<ManifestModule>,
}

/// One module's contribution to a build manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestModule {
    /// Canonical module identity (`core::collections::list`,
    /// `workspace::pkg::utils`).
    pub module: String,
    /// Span-free structural hash of the whole resolved module AST.
    pub resolved_ast_hash: [u8; 32],
    /// `blake3` of the module's canonical interface encoding.
    pub interface_hash: [u8; 32],
    /// Resolve-pass dependency modules, as canonical identities, sorted.
    pub deps: Vec<String>,
    /// Canonical object hashes this module produced (no redirects), sorted.
    pub objects: Vec<[u8; 32]>,
    /// Fully-qualified name → hash bindings, sorted by name.
    pub names: Vec<(String, [u8; 32])>,
    /// Fully-qualified name → canonical signature, sorted by name.
    pub signatures: Vec<(String, String)>,
}

impl BuildManifest {
    /// Fold a completed [`BuildResult`] into a manifest.
    ///
    /// The module set is authoritative from the build's interfaces (every
    /// registered module — core, platform, and package); a module that
    /// produced no objects (e.g. the prelude) still gets an entry so Phase 3
    /// can cache it by its embedded-source hash.
    #[must_use]
    pub fn from_build(build: &BuildResult) -> Self {
        let mut modules = Vec::with_capacity(build.interfaces.len());
        for (key, summary) in &build.interfaces {
            let output = build.module_outputs.get(key);
            modules.push(ManifestModule {
                module: key.clone(),
                resolved_ast_hash: *summary.resolved_ast_hash.as_bytes(),
                interface_hash: *summary.interface_hash.as_bytes(),
                deps: output.map(|o| o.deps.clone()).unwrap_or_default(),
                objects: output
                    .map(|o| o.objects.iter().map(|h| *h.as_bytes()).collect())
                    .unwrap_or_default(),
                names: output
                    .map(|o| {
                        o.names
                            .iter()
                            .map(|(n, h)| (n.clone(), *h.as_bytes()))
                            .collect()
                    })
                    .unwrap_or_default(),
                signatures: output
                    .map(|o| {
                        o.signatures
                            .iter()
                            .map(|(n, s)| (n.clone(), s.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        // `interfaces` is a BTreeMap, so `modules` is already key-sorted.
        Self {
            version: MANIFEST_VERSION,
            package_name: build.package_name.clone(),
            dispatch_surface_hash: *build.dispatch_surface_hash.as_bytes(),
            natives_contract_hash: *build.natives_contract_hash.as_bytes(),
            modules,
        }
    }

    /// The `blake3` hash of the manifest's canonical encoding — its identity
    /// and its file name under `meta/`.
    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        blake3::hash(&self.encode())
    }

    /// Every object hash the manifest references: each module's produced
    /// objects and every name binding's target. These plus the manifest are
    /// the gc roots that keep a snapshot loadable.
    #[must_use]
    pub fn referenced_objects(&self) -> Vec<blake3::Hash> {
        let mut out = Vec::new();
        for module in &self.modules {
            for object in &module.objects {
                out.push(blake3::Hash::from_bytes(*object));
            }
            for (_, hash) in &module.names {
                out.push(blake3::Hash::from_bytes(*hash));
            }
        }
        out
    }

    /// Encode to the canonical byte representation.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(&MANIFEST_MAGIC);
        w.buf.push(self.version);
        w.str(&self.package_name);
        w.buf.extend_from_slice(&self.dispatch_surface_hash);
        w.buf.extend_from_slice(&self.natives_contract_hash);
        w.u32(self.modules.len() as u32);
        for module in &self.modules {
            w.str(&module.module);
            w.buf.extend_from_slice(&module.resolved_ast_hash);
            w.buf.extend_from_slice(&module.interface_hash);
            w.strs(&module.deps);
            w.u32(module.objects.len() as u32);
            for object in &module.objects {
                w.buf.extend_from_slice(object);
            }
            w.u32(module.names.len() as u32);
            for (name, hash) in &module.names {
                w.str(name);
                w.buf.extend_from_slice(hash);
            }
            w.u32(module.signatures.len() as u32);
            for (name, sig) in &module.signatures {
                w.str(name);
                w.str(sig);
            }
        }
        w.buf
    }

    /// Decode from the canonical byte representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a complete, well-formed manifest
    /// of the current version (bad magic/version, truncation, invalid UTF-8,
    /// or trailing bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(4)? != MANIFEST_MAGIC {
            return Err(ManifestError::BadMagic);
        }
        let version = r.u8()?;
        if version != MANIFEST_VERSION {
            return Err(ManifestError::BadVersion(version));
        }
        let package_name = r.str()?;
        let dispatch_surface_hash = r.hash()?;
        let natives_contract_hash = r.hash()?;
        let module_count = r.u32()?;
        let mut modules = Vec::with_capacity((module_count as usize).min(r.remaining()));
        for _ in 0..module_count {
            let module = r.str()?;
            let resolved_ast_hash = r.hash()?;
            let interface_hash = r.hash()?;
            let deps = r.strs()?;
            let object_count = r.u32()?;
            let mut objects = Vec::with_capacity((object_count as usize).min(r.remaining()));
            for _ in 0..object_count {
                objects.push(r.hash()?);
            }
            let name_count = r.u32()?;
            let mut names = Vec::with_capacity((name_count as usize).min(r.remaining()));
            for _ in 0..name_count {
                names.push((r.str()?, r.hash()?));
            }
            let sig_count = r.u32()?;
            let mut signatures = Vec::with_capacity((sig_count as usize).min(r.remaining()));
            for _ in 0..sig_count {
                signatures.push((r.str()?, r.str()?));
            }
            modules.push(ManifestModule {
                module,
                resolved_ast_hash,
                interface_hash,
                deps,
                objects,
                names,
                signatures,
            });
        }
        if r.pos != bytes.len() {
            return Err(ManifestError::TrailingBytes);
        }
        Ok(Self {
            version,
            package_name,
            dispatch_surface_hash,
            natives_contract_hash,
            modules,
        })
    }
}

/// An error decoding a [`BuildManifest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// Input ended before the encoding was complete.
    Truncated,
    /// Input did not start with the manifest magic.
    BadMagic,
    /// Unknown or unsupported manifest version.
    BadVersion(u8),
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// Bytes remained after a complete manifest was decoded.
    TrailingBytes,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "build manifest is truncated"),
            Self::BadMagic => write!(f, "not a build manifest (bad magic)"),
            Self::BadVersion(v) => write!(f, "unsupported build-manifest version {v}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in build manifest"),
            Self::TrailingBytes => write!(f, "trailing bytes after build manifest"),
        }
    }
}

impl std::error::Error for ManifestError {}

// ─────────────────────────────────────────────────────────────────────────────
// Store integration
// ─────────────────────────────────────────────────────────────────────────────

impl DiskStore {
    /// Path of the manifest file for a hash, under `meta/`.
    #[must_use]
    pub fn meta_path(&self, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root()
            .join("meta")
            .join(&hex.as_str()[..2])
            .join(&hex.as_str()[2..])
    }

    /// Path of the root-pointer file.
    #[must_use]
    fn pointer_path(&self) -> PathBuf {
        self.root().join(SNAPSHOT_POINTER)
    }

    /// Persist a build manifest and repoint the root pointer at it.
    ///
    /// Crash-safety ordering: the caller must have already made every object
    /// the manifest references durable (via [`DiskStore::put_module`]). This
    /// writes the manifest bytes to `meta/` (atomic temp+rename), *then*
    /// atomically swaps the pointer. A crash before the pointer swap leaves
    /// the previous snapshot intact.
    ///
    /// Returns the manifest hash.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn write_snapshot(&self, manifest: &BuildManifest) -> Result<blake3::Hash, DiskStoreError> {
        let bytes = manifest.encode();
        let hash = blake3::hash(&bytes);
        let meta = self.meta_path(&hash);
        // Manifests are content-addressed and immutable: an identical
        // manifest already on disk is fine to leave in place.
        if !meta.exists() {
            write_atomic(self.root(), &meta, &bytes)?;
        }
        // Only now — manifest bytes durable — swap the pointer.
        let content = format!("{POINTER_PREFIX}{}\n", hash.to_hex());
        write_atomic(self.root(), &self.pointer_path(), content.as_bytes())?;
        Ok(hash)
    }

    /// The manifest hash named by the root pointer, or `None` if the pointer
    /// is absent or malformed (bad marker / bad hex). A malformed pointer is
    /// a soft miss — the build rebuilds — not an error.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer file.
    pub fn snapshot_pointer(&self) -> Result<Option<blake3::Hash>, DiskStoreError> {
        let content = match std::fs::read_to_string(self.pointer_path()) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(parse_pointer(&content))
    }

    /// Load and hash-verify the manifest stored at `hash`.
    ///
    /// # Errors
    ///
    /// Returns [`DiskStoreError::Corrupt`] if the bytes don't hash to `hash`,
    /// [`DiskStoreError::MalformedManifest`] if they don't decode, or an I/O
    /// error. A missing file is `Ok(None)`.
    pub fn load_manifest(
        &self,
        hash: &blake3::Hash,
    ) -> Result<Option<BuildManifest>, DiskStoreError> {
        let bytes = match std::fs::read(self.meta_path(hash)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let actual = blake3::hash(&bytes);
        if actual != *hash {
            return Err(DiskStoreError::Corrupt {
                expected: *hash,
                actual,
            });
        }
        let manifest = BuildManifest::decode(&bytes)
            .map_err(|e| DiskStoreError::MalformedManifest(e.to_string()))?;
        Ok(Some(manifest))
    }

    /// The current build snapshot, following the root pointer and verifying
    /// the manifest. Every failure mode — no pointer, malformed pointer,
    /// missing/corrupt/unknown-version manifest — collapses to `Ok(None)`:
    /// the build path treats a broken snapshot as no snapshot and rebuilds.
    ///
    /// # Errors
    ///
    /// Never fails for a missing or corrupt snapshot; only genuinely
    /// unexpected I/O errors (a permission failure, say) propagate.
    pub fn current_snapshot(&self) -> Result<Option<BuildManifest>, DiskStoreError> {
        let Some(hash) = self.snapshot_pointer()? else {
            return Ok(None);
        };
        match self.load_manifest(&hash) {
            Ok(manifest) => Ok(manifest),
            // A present-but-corrupt manifest is a cache miss, not an error.
            Err(DiskStoreError::Corrupt { .. } | DiskStoreError::MalformedManifest(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Diagnose the snapshot for `verify`: `None` when there is no snapshot
    /// or a healthy one, `Some(message)` when the root pointer is present but
    /// broken (unparseable, or naming a missing/corrupt/unknown-version
    /// manifest). This is the loud counterpart to [`Self::current_snapshot`]'s
    /// silent fallback.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer file.
    pub fn snapshot_health(&self) -> Result<Option<String>, DiskStoreError> {
        let content = match std::fs::read_to_string(self.pointer_path()) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let Some(hash) = parse_pointer(&content) else {
            return Ok(Some("root pointer is present but malformed".to_string()));
        };
        match self.load_manifest(&hash) {
            Ok(Some(_)) => Ok(None),
            Ok(None) => Ok(Some(format!("root pointer names missing manifest {hash}"))),
            Err(e) => Ok(Some(format!("root pointer names bad manifest {hash}: {e}"))),
        }
    }

    /// Every manifest hash with a file under `meta/`.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors. A missing `meta/` directory reads back empty.
    pub fn all_manifest_hashes(&self) -> Result<Vec<blake3::Hash>, DiskStoreError> {
        let mut hashes = Vec::new();
        let meta = self.root().join("meta");
        let entries = match std::fs::read_dir(&meta) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(hashes),
            Err(e) => return Err(e.into()),
        };
        for prefix_entry in entries {
            let prefix_entry = prefix_entry?;
            if !prefix_entry.file_type()?.is_dir() {
                continue;
            }
            let prefix = prefix_entry.file_name();
            for file_entry in std::fs::read_dir(prefix_entry.path())? {
                let file_entry = file_entry?;
                let name = file_entry.file_name();
                let hex = format!("{}{}", prefix.to_string_lossy(), name.to_string_lossy());
                if let Ok(hash) = blake3::Hash::from_hex(&hex) {
                    hashes.push(hash);
                }
            }
        }
        hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        Ok(hashes)
    }
}

/// Parse a root-pointer file's content into a manifest hash. Returns `None`
/// for anything malformed (missing marker, bad hex), which reads back as "no
/// snapshot".
fn parse_pointer(content: &str) -> Option<blake3::Hash> {
    let line = content.lines().next()?;
    let hex = line.strip_prefix(POINTER_PREFIX)?.trim();
    blake3::Hash::from_hex(hex).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Low-level writer / reader (mirrors the object/interface encoding discipline)
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

    fn strs(&mut self, items: &[String]) {
        self.u32(items.len() as u32);
        for s in items {
            self.str(s);
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

    fn take(&mut self, n: usize) -> Result<&[u8], ManifestError> {
        let end = self.pos.checked_add(n).ok_or(ManifestError::Truncated)?;
        if end > self.bytes.len() {
            return Err(ManifestError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ManifestError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ManifestError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn str(&mut self) -> Result<String, ManifestError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        std::str::from_utf8(raw)
            .map(ToString::to_string)
            .map_err(|_| ManifestError::InvalidUtf8)
    }

    fn strs(&mut self) -> Result<Vec<String>, ManifestError> {
        let count = self.u32()?;
        let mut out = Vec::with_capacity((count as usize).min(self.remaining()));
        for _ in 0..count {
            out.push(self.str()?);
        }
        Ok(out)
    }

    fn hash(&mut self) -> Result<[u8; 32], ManifestError> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }
}
