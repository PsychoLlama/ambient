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
use std::sync::Arc;

use super::{DiskStore, DiskStoreError, write_atomic};
use crate::build::BuildResult;
use crate::module_interface::ItemKindTag;

/// Magic bytes identifying an Ambient build-manifest encoding.
const MANIFEST_MAGIC: [u8; 4] = *b"ABSM";

/// Current build-manifest format version. Bumped only on an incompatible
/// encoding change; a manifest at any other version reads back as "no
/// snapshot" (and is reported by `verify`).
///
/// v2 (Phase 3): per module, the cache key and the consumed cross-module link
/// bindings (for cache-hit validation), plus the products needed to
/// reconstruct a warm build indistinguishably from cold — migrations, lambda
/// parents, and entry point; and, per package, the core+platform unit key.
///
/// v3 (Phase 6): per module, the relative source path and a structured,
/// spanned item index (functions, consts, types, traits, abilities —
/// namespace-tagged, each with its definition span and nominal identity), for
/// LSP symbol resolution and hash↔identifier debug-symbol correlation. A
/// manifest at any other version reads back as "no snapshot", which is the
/// designed cold-start behavior.
///
/// v4 (Phase 5 step 3): per module, the content hash of its persisted pre-link
/// symbolic form (`prelink/` store area), which lets a dependent absorb a
/// dependency's body-only edit by remapping the moved foreign hashes and
/// re-finalizing — no re-check, no codegen. A builtin module (cached as one
/// unit) or one that produced no relinkable body carries `None`.
pub const MANIFEST_VERSION: u8 = 4;

/// The root-pointer *directory* under the store root: one pointer file per
/// package (`snapshots/<package>`), so workspace members sharing a store
/// root their snapshots independently.
pub const SNAPSHOT_POINTER: &str = "snapshots";

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
    /// The core+platform unit cache key: matched wholesale so the entire
    /// builtin block loads from the store on a hit.
    pub core_cache_key: [u8; 32],
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
    /// This module's incremental-cache key (zero for builtin modules, which
    /// validate through [`BuildManifest::core_cache_key`]).
    pub cache_key: [u8; 32],
    /// The cross-module link bindings the module consumed, as
    /// `(rendered NameKey, final hash)`, sorted. Each must still resolve
    /// identically for a hit (the link-validation channel).
    pub consumed_links: Vec<(String, [u8; 32])>,
    /// Static `State::init_versioned` obligations `(cell, old, new)`, sorted.
    pub migrations: Vec<(String, String, String)>,
    /// Lambda hash → parent-name entries, sorted by hash.
    pub lambda_parents: Vec<([u8; 32], String)>,
    /// The module's entry-point hash, if it declares `run`.
    pub entry_point: Option<[u8; 32]>,
    /// The module's source file path relative to the package `src/`
    /// directory (`utils/format.ab`), or empty for builtin/embedded modules.
    pub source_path: String,
    /// The structured, spanned item index (v3): every top-level item,
    /// namespace-tagged with its span and identity, sorted by ident.
    pub items: Vec<ManifestItem>,
    /// The content hash of this module's persisted pre-link symbolic form
    /// (v4), under the store's `prelink/` area. `None` for builtin modules and
    /// any module with no relinkable form. A relink hit reloads this blob,
    /// remaps the moved foreign hashes, and re-finalizes.
    pub prelink: Option<[u8; 32]>,
}

/// One structured, spanned item in a module's index. The debug-symbol /
/// codebase-intelligence record: enough to resolve a name to its kind,
/// identity, and source location without loading an object (types, traits,
/// and abilities are not objects at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestItem {
    /// The item's ident path relative to its module (a single segment for a
    /// top-level item); with the enclosing [`ManifestModule::module`] this
    /// reconstructs the item's fully-qualified name.
    pub ident: Vec<String>,
    /// The precise item kind.
    pub kind: ItemKindTag,
    /// The content hash of the object the item produced — a function or const
    /// value. `None` for types/traits/abilities (not objects) and for a value
    /// item whose build recorded no binding.
    pub hash: Option<[u8; 32]>,
    /// The nominal uuid (`struct`/`enum`/`trait`/`ability`), rendered; empty
    /// otherwise.
    pub uuid: String,
    /// The definition's byte range `(start, end)` in the module source.
    pub span: (u32, u32),
    /// A one-line shape/signature summary for human inspection.
    pub summary: String,
}

impl ManifestItem {
    /// The item's own name — the last ident segment.
    #[must_use]
    pub fn name(&self) -> &str {
        self.ident.last().map_or("", String::as_str)
    }
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
            // Fold the structured index, filling each value item's object hash
            // from the module's name bindings (types/traits/abilities have no
            // object — their identity is the nominal uuid on the record).
            let names = output.map(|o| &o.names);
            let items = summary
                .items
                .iter()
                .map(|item| {
                    let hash = if item.kind.is_value() {
                        let fqn = manifest_item_fqn(key, &item.ident);
                        names.and_then(|n| n.get(&fqn)).map(|h| *h.as_bytes())
                    } else {
                        None
                    };
                    ManifestItem {
                        ident: item.ident.iter().map(ToString::to_string).collect(),
                        kind: item.kind,
                        hash,
                        uuid: item.uuid.clone(),
                        span: item.span,
                        summary: item.summary.clone(),
                    }
                })
                .collect();
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
                cache_key: output.map(|o| o.cache_key).unwrap_or_default(),
                consumed_links: output
                    .map(|o| {
                        o.consumed_links
                            .iter()
                            .map(|(n, h)| (n.clone(), *h.as_bytes()))
                            .collect()
                    })
                    .unwrap_or_default(),
                migrations: output
                    .map(|o| {
                        o.migrations
                            .iter()
                            .map(|m| (m.cell.to_string(), m.old.to_string(), m.new.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                lambda_parents: output
                    .map(|o| {
                        o.lambda_parents
                            .iter()
                            .map(|(h, p)| (*h.as_bytes(), p.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                entry_point: output.and_then(|o| o.entry_point.map(|h| *h.as_bytes())),
                source_path: summary.source_path.clone(),
                items,
                prelink: output.and_then(|o| o.prelink.map(|h| *h.as_bytes())),
            });
        }
        // `interfaces` is a BTreeMap, so `modules` is already key-sorted.
        Self {
            version: MANIFEST_VERSION,
            package_name: build.package_name.clone(),
            dispatch_surface_hash: *build.dispatch_surface_hash.as_bytes(),
            natives_contract_hash: *build.natives_contract_hash.as_bytes(),
            core_cache_key: build.core_cache_key,
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

    /// Every pre-link blob hash the manifest references (one per module that
    /// has a relinkable form). A gc root alongside [`Self::referenced_objects`]:
    /// a rooted manifest's prelink blobs must survive so a warm relink stays
    /// possible; `verify` flags any that are missing.
    #[must_use]
    pub fn referenced_prelink(&self) -> Vec<blake3::Hash> {
        self.modules
            .iter()
            .filter_map(|m| m.prelink.map(blake3::Hash::from_bytes))
            .collect()
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
        w.buf.extend_from_slice(&self.core_cache_key);
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
            w.buf.extend_from_slice(&module.cache_key);
            w.u32(module.consumed_links.len() as u32);
            for (name, hash) in &module.consumed_links {
                w.str(name);
                w.buf.extend_from_slice(hash);
            }
            w.u32(module.migrations.len() as u32);
            for (cell, old, new) in &module.migrations {
                w.str(cell);
                w.str(old);
                w.str(new);
            }
            w.u32(module.lambda_parents.len() as u32);
            for (hash, parent) in &module.lambda_parents {
                w.buf.extend_from_slice(hash);
                w.str(parent);
            }
            match &module.entry_point {
                Some(hash) => {
                    w.buf.push(1);
                    w.buf.extend_from_slice(hash);
                }
                None => w.buf.push(0),
            }
            // v3: relative source path and the structured item index.
            w.str(&module.source_path);
            w.u32(module.items.len() as u32);
            for item in &module.items {
                w.strs(&item.ident);
                w.buf.push(item.kind.as_u8());
                match &item.hash {
                    Some(hash) => {
                        w.buf.push(1);
                        w.buf.extend_from_slice(hash);
                    }
                    None => w.buf.push(0),
                }
                w.str(&item.uuid);
                w.u32(item.span.0);
                w.u32(item.span.1);
                w.str(&item.summary);
            }
            // v4: the pre-link blob hash.
            match &module.prelink {
                Some(hash) => {
                    w.buf.push(1);
                    w.buf.extend_from_slice(hash);
                }
                None => w.buf.push(0),
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
        let core_cache_key = r.hash()?;
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
            let cache_key = r.hash()?;
            let link_count = r.u32()?;
            let mut consumed_links = Vec::with_capacity((link_count as usize).min(r.remaining()));
            for _ in 0..link_count {
                consumed_links.push((r.str()?, r.hash()?));
            }
            let migration_count = r.u32()?;
            let mut migrations = Vec::with_capacity((migration_count as usize).min(r.remaining()));
            for _ in 0..migration_count {
                migrations.push((r.str()?, r.str()?, r.str()?));
            }
            let lambda_count = r.u32()?;
            let mut lambda_parents = Vec::with_capacity((lambda_count as usize).min(r.remaining()));
            for _ in 0..lambda_count {
                lambda_parents.push((r.hash()?, r.str()?));
            }
            let entry_point = match r.u8()? {
                0 => None,
                1 => Some(r.hash()?),
                other => return Err(ManifestError::BadTag(other)),
            };
            let source_path = r.str()?;
            let item_count = r.u32()?;
            let mut items = Vec::with_capacity((item_count as usize).min(r.remaining()));
            for _ in 0..item_count {
                items.push(decode_item(&mut r)?);
            }
            let prelink = match r.u8()? {
                0 => None,
                1 => Some(r.hash()?),
                other => return Err(ManifestError::BadTag(other)),
            };
            modules.push(ManifestModule {
                module,
                resolved_ast_hash,
                interface_hash,
                deps,
                objects,
                names,
                signatures,
                cache_key,
                consumed_links,
                migrations,
                lambda_parents,
                entry_point,
                source_path,
                items,
                prelink,
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
            core_cache_key,
            modules,
        })
    }
}

/// Decode one structured item record (v3).
fn decode_item(r: &mut Reader<'_>) -> Result<ManifestItem, ManifestError> {
    let ident = r.strs()?;
    let kind_byte = r.u8()?;
    let kind = ItemKindTag::from_u8(kind_byte).ok_or(ManifestError::BadTag(kind_byte))?;
    let hash = match r.u8()? {
        0 => None,
        1 => Some(r.hash()?),
        other => return Err(ManifestError::BadTag(other)),
    };
    let uuid = r.str()?;
    let span = (r.u32()?, r.u32()?);
    let summary = r.str()?;
    Ok(ManifestItem {
        ident,
        kind,
        hash,
        uuid,
        span,
        summary,
    })
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
    /// An unexpected discriminant byte (e.g. an `Option` tag).
    BadTag(u8),
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
            Self::BadTag(t) => write!(f, "unexpected tag {t} in build manifest"),
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

    /// Path of one package's root-pointer file. Pointers live per package
    /// under `snapshots/` — workspace members share one store, and each
    /// member's latest build roots its own snapshot independently.
    #[must_use]
    fn pointer_path(&self, package: &str) -> PathBuf {
        self.root().join(SNAPSHOT_POINTER).join(package)
    }

    /// Persist a build manifest and repoint its package's root pointer at it.
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
        write_atomic(
            self.root(),
            &self.pointer_path(&manifest.package_name),
            content.as_bytes(),
        )?;
        Ok(hash)
    }

    /// Every package's root pointer, sorted by package name. Malformed
    /// pointer files are skipped (a soft miss, never an error).
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer directory.
    pub fn snapshot_pointers(&self) -> Result<Vec<(String, blake3::Hash)>, DiskStoreError> {
        let dir = self.root().join(SNAPSHOT_POINTER);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut pointers = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(package) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if let Some(hash) = parse_pointer(&content) {
                pointers.push((package, hash));
            }
        }
        pointers.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(pointers)
    }

    /// The manifest hash named by `package`'s root pointer, or `None` if the
    /// pointer is absent or malformed (bad marker / bad hex). A malformed
    /// pointer is a soft miss — the build rebuilds — not an error.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer file.
    pub fn snapshot_pointer_for(
        &self,
        package: &str,
    ) -> Result<Option<blake3::Hash>, DiskStoreError> {
        let content = match std::fs::read_to_string(self.pointer_path(package)) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(parse_pointer(&content))
    }

    /// The sole (or first, by package name) root pointer — the single-package
    /// store's view. Multi-package consumers use [`Self::snapshot_pointers`].
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer directory.
    pub fn snapshot_pointer(&self) -> Result<Option<blake3::Hash>, DiskStoreError> {
        Ok(self.snapshot_pointers()?.into_iter().next().map(|(_, h)| h))
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

    /// The current build snapshot, following the sole (or first) root
    /// pointer and verifying the manifest. Every failure mode — no pointer,
    /// malformed pointer, missing/corrupt/unknown-version manifest —
    /// collapses to `Ok(None)`: the build path treats a broken snapshot as
    /// no snapshot and rebuilds.
    ///
    /// # Errors
    ///
    /// Never fails for a missing or corrupt snapshot; only genuinely
    /// unexpected I/O errors (a permission failure, say) propagate.
    pub fn current_snapshot(&self) -> Result<Option<BuildManifest>, DiskStoreError> {
        let Some(hash) = self.snapshot_pointer()? else {
            return Ok(None);
        };
        self.manifest_or_miss(hash)
    }

    /// The named package's current build snapshot —
    /// [`Self::current_snapshot`] for one pointer of a multi-package
    /// (workspace) store. The same failure modes collapse to `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Never fails for a missing or corrupt snapshot; only genuinely
    /// unexpected I/O errors propagate.
    pub fn current_snapshot_for(
        &self,
        package: &str,
    ) -> Result<Option<BuildManifest>, DiskStoreError> {
        let Some(hash) = self.snapshot_pointer_for(package)? else {
            return Ok(None);
        };
        self.manifest_or_miss(hash)
    }

    /// Every package's current snapshot, deduplicated by manifest hash (a
    /// workspace-root build points several packages at one manifest). Broken
    /// pointers and manifests are skipped, exactly like
    /// [`Self::current_snapshot`].
    ///
    /// # Errors
    ///
    /// Only genuinely unexpected I/O errors propagate.
    pub fn current_snapshots(&self) -> Result<Vec<BuildManifest>, DiskStoreError> {
        let mut seen: std::collections::HashSet<blake3::Hash> = std::collections::HashSet::new();
        let mut manifests = Vec::new();
        for (_, hash) in self.snapshot_pointers()? {
            if !seen.insert(hash) {
                continue;
            }
            if let Some(manifest) = self.manifest_or_miss(hash)? {
                manifests.push(manifest);
            }
        }
        Ok(manifests)
    }

    /// Load a manifest, collapsing corrupt/malformed to a soft miss.
    fn manifest_or_miss(
        &self,
        hash: blake3::Hash,
    ) -> Result<Option<BuildManifest>, DiskStoreError> {
        match self.load_manifest(&hash) {
            Ok(manifest) => Ok(manifest),
            // A present-but-corrupt manifest is a cache miss, not an error.
            Err(DiskStoreError::Corrupt { .. } | DiskStoreError::MalformedManifest(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Diagnose the snapshots for `verify`: `None` when every pointer is
    /// absent or healthy, `Some(message)` when some package's root pointer
    /// is present but broken (unparseable, or naming a
    /// missing/corrupt/unknown-version manifest). This is the loud
    /// counterpart to [`Self::current_snapshot`]'s silent fallback.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the pointer files.
    pub fn snapshot_health(&self) -> Result<Option<String>, DiskStoreError> {
        let dir = self.root().join(SNAPSHOT_POINTER);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let package = entry.file_name().to_string_lossy().to_string();
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let Some(hash) = parse_pointer(&content) else {
                return Ok(Some(format!(
                    "root pointer for `{package}` is present but malformed"
                )));
            };
            match self.load_manifest(&hash) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Ok(Some(format!(
                        "root pointer for `{package}` names missing manifest {hash}"
                    )));
                }
                Err(e) => {
                    return Ok(Some(format!(
                        "root pointer for `{package}` names bad manifest {hash}: {e}"
                    )));
                }
            }
        }
        Ok(None)
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

/// The fully-qualified name of a structured item, for looking its object
/// hash up in the module's name bindings: the module identity string joined
/// with the ident path (`workspace::pkg::main` + `["run"]` →
/// `workspace::pkg::main::run`).
fn manifest_item_fqn(module: &str, ident: &[Arc<str>]) -> String {
    let mut out = String::from(module);
    for segment in ident {
        out.push_str("::");
        out.push_str(segment);
    }
    out
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
