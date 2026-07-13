//! Persistent content-addressed store, laid out on the filesystem.
//!
//! Ambient's ethos is that language semantics map onto the filesystem, so
//! the store is plain files, git-style — not a database:
//!
//! ```text
//! <root>/
//!   format                    "ambient-store-v1\n" (version marker)
//!   names                     "<hex-hash> <name>\n" per binding, sorted
//!   signatures                "<name>\t<canonical-signature>\n", sorted
//!   migrations                "<cell>\t<old>\t<new>\n" per obligation, sorted
//!   objects/<2hex>/<62hex>    one canonical object per file
//!   meta/<2hex>/<62hex>       one build manifest (snapshot) per file
//!   snapshot                  root pointer to the current manifest
//!   tags/<name>               "ambient-tag-v1 <hex>\n": a named manifest
//! ```
//!
//! The `meta/` manifests and the `snapshot` pointer are the build-snapshot
//! layer (`snapshot.rs`); `tags/` names manifests to keep and diff against
//! (`tags.rs`); `diff.rs` compares two manifests. The flat `names` file stays
//! the authoritative value-binding index (gc roots, deploy, existing
//! subcommands); the manifest's structured item index is additive.
//!
//! `signatures` and `migrations` are the two sections artifact packs
//! gained in pack v2 (`CompiledModule::to_pack`), persisted so a future
//! "deploy from the package store" frontend can reconstruct a generation's
//! name bindings (`hash + signature`, the rebinding rule's input) and its
//! pre-swap migration obligations. Both are sidecars beside `names`: an old
//! store simply lacks the files, which reads back as "no signatures / no
//! migrations" — and a missing signature fails safe to retire-and-fresh
//! (see `ref/live-upgrade.md`). Their fields are tab-separated with a
//! minimal `\`/newline/tab escape, because a signature or cell name may
//! contain spaces (`(Number) -> String with row`) — the flat `names`
//! file's space delimiter would be ambiguous there.
//!
//! Every object file's path is derived from its content: for plain and
//! group objects, `blake3(file bytes)` *is* the file name, so a read
//! verifies itself and corruption is always detected. Redirect files (which
//! live at a group member's derived hash) are verified by loading their
//! group and re-deriving the member hash.
//!
//! Objects are immutable, so writes need no locking: content is written to
//! a temporary file and atomically renamed into place. Two processes
//! writing the same object race benignly — the content is identical. The
//! `names` file is the only mutable state and is replaced atomically;
//! last-writer-wins is correct because it always reflects a complete build.
//!
//! This layout is also the transport story: syncing a store is copying
//! files, and a peer can verify everything it received without trusting
//! the sender.

// Group member counts are bounded by the u32 object encoding.
#![allow(clippy::cast_possible_truncation)]

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bytecode::CompiledFunction;
use crate::compiler::{CompiledModule, MigrationRecord};
use crate::object::{ObjectError, StoredObject};
use crate::store::Store;

/// Contents of the `format` marker file.
pub const FORMAT_MARKER: &str = "ambient-store-v1\n";

/// Counter for unique temp file names within a process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Errors from disk store operations.
#[derive(Debug)]
pub enum DiskStoreError {
    /// Underlying I/O failure.
    Io(io::Error),
    /// An object file failed to decode.
    Object {
        hash: blake3::Hash,
        error: ObjectError,
    },
    /// An object file's content does not hash to its path.
    Corrupt {
        expected: blake3::Hash,
        actual: blake3::Hash,
    },
    /// A redirect points at a group that doesn't contain it.
    BadRedirect {
        member: blake3::Hash,
        group: blake3::Hash,
    },
    /// The store directory belongs to an unknown format version.
    UnknownFormat(String),
    /// The `names` file is malformed.
    MalformedNames(String),
    /// The `signatures` file is malformed.
    MalformedSignatures(String),
    /// The `migrations` file is malformed.
    MalformedMigrations(String),
    /// A build manifest under `meta/` failed to decode.
    MalformedManifest(String),
    /// A tag name failed the conservative charset/length rules.
    InvalidTagName(String),
}

impl std::fmt::Display for DiskStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "store I/O error: {e}"),
            Self::Object { hash, error } => write!(f, "object {hash} is malformed: {error}"),
            Self::Corrupt { expected, actual } => write!(
                f,
                "object file is corrupt: stored at {expected} but content hashes to {actual}"
            ),
            Self::BadRedirect { member, group } => write!(
                f,
                "redirect at {member} points at group {group}, which does not derive it"
            ),
            Self::UnknownFormat(found) => {
                write!(
                    f,
                    "unknown store format {found:?} (expected {FORMAT_MARKER:?})"
                )
            }
            Self::MalformedNames(line) => write!(f, "malformed names entry: {line:?}"),
            Self::MalformedSignatures(line) => {
                write!(f, "malformed signatures entry: {line:?}")
            }
            Self::MalformedMigrations(line) => {
                write!(f, "malformed migrations entry: {line:?}")
            }
            Self::MalformedManifest(msg) => write!(f, "malformed build manifest: {msg}"),
            Self::InvalidTagName(name) => write!(
                f,
                "invalid tag name {name:?} (use letters, digits, `.`, `_`, `-`)"
            ),
        }
    }
}

impl std::error::Error for DiskStoreError {}

impl From<io::Error> for DiskStoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Statistics from writing a module to the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PutStats {
    /// Objects newly written.
    pub written: usize,
    /// Objects already present (deduplicated).
    pub deduplicated: usize,
}

/// Result of verifying every object in the store.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// Objects (and manifests) that decoded and hashed correctly.
    pub valid: usize,
    /// Files whose content did not match their path, with the error.
    pub corrupt: Vec<(blake3::Hash, String)>,
    /// External references that no object in the store provides.
    pub dangling: Vec<blake3::Hash>,
    /// Set when the snapshot root pointer names a manifest that is missing,
    /// corrupt, or of an unknown version — a broken snapshot the build path
    /// silently ignores but `verify` flags loudly. Carries a human message.
    pub dangling_snapshot: Option<String>,
    /// Tags naming a missing/corrupt manifest (or a malformed tag file), as
    /// `(tag name, reason)`, sorted by name. A tag pins a manifest against
    /// gc, so a dangling tag is a real integrity fault `verify` flags.
    pub bad_tags: Vec<(String, String)>,
}

impl VerifyReport {
    /// True when every object is valid, every reference resolves, the
    /// snapshot pointer (if any) resolves to a good manifest, and every tag
    /// resolves to a good manifest.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.corrupt.is_empty()
            && self.dangling.is_empty()
            && self.dangling_snapshot.is_none()
            && self.bad_tags.is_empty()
    }
}

/// A content-addressed store persisted as plain files.
#[derive(Debug, Clone)]
pub struct DiskStore {
    root: PathBuf,
}

impl DiskStore {
    /// Open (or initialize) a store at `root`.
    ///
    /// # Errors
    ///
    /// Fails if directories can't be created or the format marker belongs
    /// to a different version.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, DiskStoreError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("objects"))?;

        let format_path = root.join("format");
        match std::fs::read_to_string(&format_path) {
            Ok(found) => {
                if found != FORMAT_MARKER {
                    return Err(DiskStoreError::UnknownFormat(found));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                write_atomic(&root, &format_path, FORMAT_MARKER.as_bytes())?;
            }
            Err(e) => return Err(e.into()),
        }

        Ok(Self { root })
    }

    /// The store's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the object file for a hash.
    #[must_use]
    pub fn object_path(&self, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root
            .join("objects")
            .join(&hex.as_str()[..2])
            .join(&hex.as_str()[2..])
    }

    /// Whether an object file exists for this hash.
    #[must_use]
    pub fn contains(&self, hash: &blake3::Hash) -> bool {
        self.object_path(hash).exists()
    }

    /// Write one object. Returns `true` if it was newly written.
    ///
    /// For plain and group objects the file lands at the object hash. For
    /// redirects, `at` names the member hash the redirect stands for.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn put_object_at(
        &self,
        at: &blake3::Hash,
        object: &StoredObject,
    ) -> Result<bool, DiskStoreError> {
        let path = self.object_path(at);
        if path.exists() {
            return Ok(false);
        }
        write_atomic(&self.root, &path, &object.encode())?;
        Ok(true)
    }

    /// Write a plain or group object at its content hash.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn put_object(&self, object: &StoredObject) -> Result<blake3::Hash, DiskStoreError> {
        let hash = object.hash();
        self.put_object_at(&hash, object)?;
        Ok(hash)
    }

    /// Persist every object of a compiled module and bind its names.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn put_module(&self, module: &CompiledModule) -> Result<PutStats, DiskStoreError> {
        let mut stats = PutStats::default();
        for (hash, object) in &module.objects {
            if self.put_object_at(hash, object)? {
                stats.written += 1;
            } else {
                stats.deduplicated += 1;
            }
        }

        // Merge this module's bindings into the names index. Functions and
        // consts share the flat index; the object kind at each hash tells
        // them apart (`store show`/`ls`).
        let mut names = self.names()?;
        let mut signatures = self.signatures()?;
        for (name, hash) in module.function_names.iter().chain(&module.const_names) {
            names.insert(name.to_string(), *hash);
            // Keep signatures in lockstep with names: a name this build
            // rebinds either gets its fresh rendering or drops the stale
            // one. A stale signature is worse than none — `None` fails safe
            // to retire-and-fresh, but a wrong rendering could make the
            // deploy rule *rebind* incompatibly.
            match module.signatures.get(name.as_ref()) {
                Some(sig) => {
                    signatures.insert(name.to_string(), sig.to_string());
                }
                None => {
                    signatures.remove(name.as_ref());
                }
            }
        }
        self.write_names(&names)?;
        self.write_signatures(&signatures)?;

        // Migrations are per-build obligations, not per-name facts, so a
        // complete build replaces them wholesale — matching the names
        // file's "always reflects a complete build" contract. (Incremental
        // per-module puts therefore do *not* accumulate obligations; put a
        // merged build.)
        self.write_migrations(&module.migrations)?;

        Ok(stats)
    }

    /// Load and verify the object stored at a hash.
    ///
    /// Plain/group objects are verified by re-hashing their bytes.
    /// Redirects are returned as-is; use [`DiskStore::get_function`] to
    /// follow and verify them.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors, undecodable files, or corruption.
    pub fn get_object(&self, hash: &blake3::Hash) -> Result<Option<StoredObject>, DiskStoreError> {
        let path = self.object_path(hash);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let object = StoredObject::decode(&bytes)
            .map_err(|error| DiskStoreError::Object { hash: *hash, error })?;
        if !matches!(object, StoredObject::Redirect { .. }) {
            let actual = blake3::hash(&bytes);
            if actual != *hash {
                return Err(DiskStoreError::Corrupt {
                    expected: *hash,
                    actual,
                });
            }
        }
        Ok(Some(object))
    }

    /// Load a single function by hash, following redirects and verifying
    /// everything on the way.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors, corruption, or redirects whose group does not
    /// actually derive the requested hash.
    pub fn get_function(
        &self,
        hash: &blake3::Hash,
    ) -> Result<Option<CompiledFunction>, DiskStoreError> {
        let Some(object) = self.get_object(hash)? else {
            return Ok(None);
        };
        let (group_hash, object) = match object {
            StoredObject::Redirect { group, .. } => {
                let Some(group_object) = self.get_object(&group)? else {
                    return Ok(None);
                };
                (group, group_object)
            }
            other => (*hash, other),
        };
        let materialized = object
            .materialize()
            .map_err(|error| DiskStoreError::Object {
                hash: group_hash,
                error,
            })?;
        match materialized.into_iter().find(|(h, _)| h == hash) {
            Some((_, func)) => Ok(Some(func)),
            None => Err(DiskStoreError::BadRedirect {
                member: *hash,
                group: group_hash,
            }),
        }
    }

    /// Load a function and its full transitive dependency closure into an
    /// in-memory [`Store`]. Returns the number of functions loaded.
    ///
    /// # Errors
    ///
    /// Fails if any function in the closure is missing or corrupt.
    pub fn load_closure(
        &self,
        root: &blake3::Hash,
        store: &mut Store,
    ) -> Result<usize, DiskStoreError> {
        let mut loaded = 0;
        let mut pending = vec![*root];
        let mut visited: HashSet<blake3::Hash> = HashSet::new();

        while let Some(hash) = pending.pop() {
            if !visited.insert(hash) || store.contains(&hash) {
                continue;
            }
            let Some(object) = self.get_object(&hash)? else {
                return Err(DiskStoreError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("missing object for {hash}"),
                )));
            };
            let object = match object {
                StoredObject::Redirect { group, .. } => {
                    self.get_object(&group)?.ok_or_else(|| {
                        DiskStoreError::Io(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("missing group {group} for member {hash}"),
                        ))
                    })?
                }
                other => other,
            };
            let materialized = object
                .materialize()
                .map_err(|error| DiskStoreError::Object { hash, error })?;
            for (_, func) in &materialized {
                pending.extend(func.referenced_hashes());
            }
            loaded += materialized.len();
            store
                .add_object(object)
                .map_err(|_| DiskStoreError::Io(io::Error::other("object rejected by store")))?;
        }

        Ok(loaded)
    }

    // ─────────────────────────────────────────────────────────────────────
    // Names index
    // ─────────────────────────────────────────────────────────────────────

    /// Read the name → hash bindings from the latest build.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a malformed names file.
    pub fn names(&self) -> Result<BTreeMap<String, blake3::Hash>, DiskStoreError> {
        let path = self.root.join("names");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        let mut names = BTreeMap::new();
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((hex, name)) = line.split_once(' ') else {
                return Err(DiskStoreError::MalformedNames(line.to_string()));
            };
            let hash = blake3::Hash::from_hex(hex)
                .map_err(|_| DiskStoreError::MalformedNames(line.to_string()))?;
            names.insert(name.to_string(), hash);
        }
        Ok(names)
    }

    /// Atomically replace the names index.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn write_names(
        &self,
        names: &BTreeMap<String, blake3::Hash>,
    ) -> Result<(), DiskStoreError> {
        let mut content = String::new();
        for (name, hash) in names {
            content.push_str(&hash.to_hex());
            content.push(' ');
            content.push_str(name);
            content.push('\n');
        }
        write_atomic(&self.root, &self.root.join("names"), content.as_bytes())?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Signatures and migrations (pack v2 parity)
    // ─────────────────────────────────────────────────────────────────────

    /// Read the name → canonical-signature renderings from the latest
    /// build. This is a *subset* of [`Self::names`]: a name whose producer
    /// rendered no signature has no entry here (fails safe to
    /// retire-and-fresh under the rebinding rule). An old store lacking the
    /// file reads back empty.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a malformed signatures file.
    pub fn signatures(&self) -> Result<BTreeMap<String, String>, DiskStoreError> {
        let path = self.root.join("signatures");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        let mut signatures = BTreeMap::new();
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((name, sig)) = line.split_once('\t') else {
                return Err(DiskStoreError::MalformedSignatures(line.to_string()));
            };
            signatures.insert(
                unescape_field(name)
                    .ok_or_else(|| DiskStoreError::MalformedSignatures(line.to_string()))?,
                unescape_field(sig)
                    .ok_or_else(|| DiskStoreError::MalformedSignatures(line.to_string()))?,
            );
        }
        Ok(signatures)
    }

    /// Atomically replace the signatures sidecar.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn write_signatures(
        &self,
        signatures: &BTreeMap<String, String>,
    ) -> Result<(), DiskStoreError> {
        let mut content = String::new();
        for (name, sig) in signatures {
            content.push_str(&escape_field(name));
            content.push('\t');
            content.push_str(&escape_field(sig));
            content.push('\n');
        }
        write_atomic(
            &self.root,
            &self.root.join("signatures"),
            content.as_bytes(),
        )?;
        Ok(())
    }

    /// Read the latest build's statically-named `State::init_versioned`
    /// obligations. An old store lacking the file reads back empty. The
    /// result is sorted (by `(cell, old, new)`), matching what was written.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a malformed migrations file.
    pub fn migrations(&self) -> Result<Vec<MigrationRecord>, DiskStoreError> {
        let path = self.root.join("migrations");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut migrations = Vec::new();
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split('\t');
            let (Some(cell), Some(old), Some(new), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(DiskStoreError::MalformedMigrations(line.to_string()));
            };
            let malformed = || DiskStoreError::MalformedMigrations(line.to_string());
            migrations.push(MigrationRecord {
                cell: unescape_field(cell).ok_or_else(malformed)?.into(),
                old: unescape_field(old).ok_or_else(malformed)?.into(),
                new: unescape_field(new).ok_or_else(malformed)?.into(),
            });
        }
        Ok(migrations)
    }

    /// Atomically replace the migrations sidecar with a complete build's
    /// obligations, written sorted for determinism.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn write_migrations(&self, migrations: &[MigrationRecord]) -> Result<(), DiskStoreError> {
        let mut sorted: Vec<&MigrationRecord> = migrations.iter().collect();
        sorted.sort_by(|a, b| (&a.cell, &a.old, &a.new).cmp(&(&b.cell, &b.old, &b.new)));
        let mut content = String::new();
        for record in sorted {
            content.push_str(&escape_field(&record.cell));
            content.push('\t');
            content.push_str(&escape_field(&record.old));
            content.push('\t');
            content.push_str(&escape_field(&record.new));
            content.push('\n');
        }
        write_atomic(
            &self.root,
            &self.root.join("migrations"),
            content.as_bytes(),
        )?;
        Ok(())
    }

    /// Convenience: every name binding as `name → (hash, signature)`,
    /// joining the `names` index with the `signatures` sidecar. This is the
    /// exact shape a deploy frontend needs to reconstruct a generation's
    /// `Binding`s (see `crates/ambient-platform/src/deploy.rs`); a name with
    /// no rendered signature carries `None`, which fails safe to
    /// retire-and-fresh.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a malformed names/signatures file.
    pub fn name_bindings(
        &self,
    ) -> Result<BTreeMap<String, (blake3::Hash, Option<String>)>, DiskStoreError> {
        let names = self.names()?;
        let mut signatures = self.signatures()?;
        Ok(names
            .into_iter()
            .map(|(name, hash)| {
                let sig = signatures.remove(&name);
                (name, (hash, sig))
            })
            .collect())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Maintenance
    // ─────────────────────────────────────────────────────────────────────

    /// Every hash with an object file in the store (including redirects).
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn all_hashes(&self) -> Result<Vec<blake3::Hash>, DiskStoreError> {
        let mut hashes = Vec::new();
        let objects = self.root.join("objects");
        for prefix_entry in std::fs::read_dir(&objects)? {
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

    /// Verify every object in the store: decode, check content hashes,
    /// check redirects against their groups, and report dangling
    /// references.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors (individual corrupt objects are *reported*, not
    /// errors).
    pub fn verify(&self) -> Result<VerifyReport, DiskStoreError> {
        let mut report = VerifyReport::default();
        let hashes = self.all_hashes()?;
        let present: HashSet<blake3::Hash> = hashes.iter().copied().collect();
        let mut referenced: HashSet<blake3::Hash> = HashSet::new();

        for hash in &hashes {
            match self.get_object(hash) {
                Ok(Some(StoredObject::Redirect { group, index })) => {
                    // Valid iff the group exists and derives this hash.
                    match self.get_object(&group) {
                        Ok(Some(StoredObject::Group(members))) => {
                            let derived =
                                crate::object::member_hash(&group, index, members.len() as u32);
                            if derived == *hash {
                                report.valid += 1;
                            } else {
                                report.corrupt.push((
                                    *hash,
                                    format!("redirect derives {derived}, not its own path"),
                                ));
                            }
                        }
                        Ok(_) => report
                            .corrupt
                            .push((*hash, format!("redirect target {group} is not a group"))),
                        Err(e) => report.corrupt.push((*hash, e.to_string())),
                    }
                }
                Ok(Some(object)) => {
                    report.valid += 1;
                    match object.materialize() {
                        Ok(materialized) => {
                            for (_, func) in materialized {
                                referenced.extend(func.referenced_hashes());
                            }
                        }
                        Err(e) => report.corrupt.push((*hash, e.to_string())),
                    }
                }
                Ok(None) => {}
                Err(e) => report.corrupt.push((*hash, e.to_string())),
            }
        }

        for dep in referenced {
            if !present.contains(&dep) {
                report.dangling.push(dep);
            }
        }
        report
            .dangling
            .sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

        // Manifests under `meta/`: decode and re-hash every one, and check
        // that the root pointer (if present) resolves to a good manifest.
        for hash in self.all_manifest_hashes()? {
            match self.load_manifest(&hash) {
                Ok(Some(_)) => report.valid += 1,
                Ok(None) => {} // vanished between listing and load; ignore
                Err(e) => report.corrupt.push((hash, e.to_string())),
            }
        }
        report.dangling_snapshot = self.snapshot_health()?;
        report.bad_tags = self.tag_health()?;
        Ok(report)
    }

    /// Delete every object not reachable from the given roots (plus the
    /// names index, which always counts as roots). Returns the number of
    /// files removed.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn gc(&self, extra_roots: &[blake3::Hash]) -> Result<usize, DiskStoreError> {
        let mut reachable: HashSet<blake3::Hash> = HashSet::new();
        let mut pending: Vec<blake3::Hash> = extra_roots.to_vec();
        pending.extend(self.names()?.values().copied());

        // The current snapshot and every tag are gc roots: every object a
        // rooted manifest references must survive so the snapshot stays
        // loadable. A broken snapshot/tag roots nothing (it protects no
        // objects, and its pointer is already treated as absent). Collect the
        // manifest hashes to keep alongside their objects.
        let mut keep_manifests: HashSet<blake3::Hash> = HashSet::new();
        let current_manifest = self.current_snapshot()?;
        if let Some(manifest) = &current_manifest {
            keep_manifests.insert(manifest.hash());
            pending.extend(manifest.referenced_objects());
        }
        for tagged in self.tagged_manifest_hashes()? {
            if let Some(manifest) = self.load_manifest(&tagged)? {
                keep_manifests.insert(tagged);
                pending.extend(manifest.referenced_objects());
            }
        }

        while let Some(hash) = pending.pop() {
            if !reachable.insert(hash) {
                continue;
            }
            let Some(object) = self.get_object(&hash)? else {
                continue;
            };
            let object = match object {
                StoredObject::Redirect { group, .. } => {
                    reachable.insert(group);
                    match self.get_object(&group)? {
                        Some(g) => g,
                        None => continue,
                    }
                }
                other => other,
            };
            if let Ok(materialized) = object.materialize() {
                for (member_hash, func) in materialized {
                    // Member hashes stay reachable (their redirect files).
                    reachable.insert(member_hash);
                    pending.extend(func.referenced_hashes());
                }
            }
        }

        let mut removed = 0;
        for hash in self.all_hashes()? {
            if !reachable.contains(&hash) {
                std::fs::remove_file(self.object_path(&hash))?;
                removed += 1;
            }
        }

        // Prune stale manifests under `meta/`: keep the one the pointer names
        // and every tagged one (all loaded and protected above). Any other
        // manifest is a superseded build's leftover.
        for hash in self.all_manifest_hashes()? {
            if !keep_manifests.contains(&hash) {
                std::fs::remove_file(self.meta_path(&hash))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Write bytes to `path` atomically: write a temp file in the store root,
/// then rename into place. Rename is atomic on POSIX filesystems, so
/// readers never observe partial content.
fn write_atomic(store_root: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = store_root.join(format!(
        "tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Escape a `signatures`/`migrations` field so tab (the field separator)
/// and newline (the record separator) can never appear raw. Canonical
/// signature renderings are single-line today, but a cell name is an
/// arbitrary user string literal and a fingerprint an arbitrary rendering,
/// so the store escapes defensively rather than assume. Backslash is the
/// escape lead; the common case (no special chars) is left untouched and
/// stays human-readable.
fn escape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Reverse [`escape_field`]. Returns `None` on a malformed escape sequence
/// (a trailing backslash or an unknown escape), which the caller reports as
/// a malformed sidecar entry.
fn unescape_field(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            _ => return None,
        }
    }
    Some(out)
}

mod snapshot;
pub use snapshot::{
    BuildManifest, MANIFEST_VERSION, ManifestError, ManifestItem, ManifestModule, SNAPSHOT_POINTER,
};

mod tags;
pub use tags::is_valid_tag_name;

mod diff;
pub use diff::{
    BindingChange, BindingDiff, ModuleChange, ModuleDiff, ObjectDiff, SnapshotDiff,
    classify_binding, diff_manifests,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod tags_tests;

#[cfg(test)]
mod diff_tests;
