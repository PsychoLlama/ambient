//! Persistent content-addressed store, laid out on the filesystem.
//!
//! Ambient's ethos is that language semantics map onto the filesystem, so
//! the store is plain files, git-style — not a database:
//!
//! ```text
//! <root>/
//!   format                    "ambient-store-v1\n" (version marker)
//!   names                     "<hex-hash> <name>\n" per binding, sorted
//!   objects/<2hex>/<62hex>    one canonical object per file
//! ```
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
use crate::compiler::CompiledModule;
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
    /// Objects that decoded and hashed correctly.
    pub valid: usize,
    /// Files whose content did not match their path, with the error.
    pub corrupt: Vec<(blake3::Hash, String)>,
    /// External references that no object in the store provides.
    pub dangling: Vec<blake3::Hash>,
}

impl VerifyReport {
    /// True when every object is valid and every reference resolves.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.corrupt.is_empty() && self.dangling.is_empty()
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

        // Merge this module's bindings into the names index.
        let mut names = self.names()?;
        for (name, hash) in &module.function_names {
            names.insert(name.to_string(), *hash);
        }
        self.write_names(&names)?;

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
                for dep in &func.dependencies {
                    pending.push(*dep);
                }
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
                                referenced.extend(func.dependencies.iter().copied());
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
                    pending.extend(func.dependencies.iter().copied());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{GroupMember, ObjectConstant, ObjectFunction, ObjectRef};

    fn temp_store() -> (tempfile::TempDir, DiskStore) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = DiskStore::open(dir.path().join("store")).expect("open store");
        (dir, store)
    }

    fn plain(n: f64) -> StoredObject {
        StoredObject::Plain(ObjectFunction {
            bytecode: vec![1, 2, 3],
            constants: vec![ObjectConstant::Number(n)],
            local_count: 0,
            param_count: 0,
            dependencies: vec![],
        })
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, store) = temp_store();
        let object = plain(42.0);
        let hash = store.put_object(&object).expect("put");
        assert!(store.contains(&hash));
        let loaded = store.get_object(&hash).expect("get").expect("present");
        assert_eq!(loaded, object);
    }

    #[test]
    fn missing_object_is_none() {
        let (_dir, store) = temp_store();
        let absent = blake3::hash(b"nothing here");
        assert!(store.get_object(&absent).expect("get").is_none());
        assert!(store.get_function(&absent).expect("get").is_none());
    }

    #[test]
    fn corruption_is_detected() {
        let (_dir, store) = temp_store();
        let hash = store.put_object(&plain(1.0)).expect("put");

        // Flip one byte in the object file.
        let path = store.object_path(&hash);
        let mut bytes = std::fs::read(&path).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).expect("write");

        let result = store.get_object(&hash);
        assert!(
            matches!(
                result,
                Err(DiskStoreError::Corrupt { .. } | DiskStoreError::Object { .. })
            ),
            "corrupted object must not load: {result:?}"
        );
    }

    #[test]
    fn group_and_redirects_roundtrip_through_disk() {
        let (_dir, store) = temp_store();

        let member = |other: u32, name: &str| GroupMember {
            name: Some(name.to_string()),
            function: ObjectFunction {
                bytecode: vec![7],
                constants: vec![ObjectConstant::Ref(ObjectRef::Internal(other))],
                local_count: 0,
                param_count: 1,
                dependencies: vec![ObjectRef::Internal(other)],
            },
        };
        let group = StoredObject::Group(vec![member(1, "even"), member(0, "odd")]);
        let group_hash = store.put_object(&group).expect("put group");

        let even = crate::object::member_hash(&group_hash, 0, 2);
        let odd = crate::object::member_hash(&group_hash, 1, 2);
        store
            .put_object_at(
                &even,
                &StoredObject::Redirect {
                    group: group_hash,
                    index: 0,
                },
            )
            .expect("put redirect");
        store
            .put_object_at(
                &odd,
                &StoredObject::Redirect {
                    group: group_hash,
                    index: 1,
                },
            )
            .expect("put redirect");

        // get_function follows the redirect, verifies, and substitutes
        // sibling hashes.
        let func = store.get_function(&even).expect("get").expect("present");
        assert_eq!(func.hash, even);
        assert_eq!(func.dependencies, vec![odd]);
    }

    #[test]
    fn lying_redirect_is_rejected() {
        let (_dir, store) = temp_store();
        let decoy_hash = store.put_object(&plain(3.0)).expect("put");

        // A redirect claiming that some arbitrary hash is member 5 of a
        // plain object's hash.
        let liar = blake3::hash(b"liar");
        store
            .put_object_at(
                &liar,
                &StoredObject::Redirect {
                    group: decoy_hash,
                    index: 5,
                },
            )
            .expect("put");

        let result = store.get_function(&liar);
        assert!(
            matches!(result, Err(DiskStoreError::BadRedirect { .. })),
            "redirect to a non-deriving group must fail: {result:?}"
        );
    }

    #[test]
    fn names_roundtrip_and_merge() {
        let (_dir, store) = temp_store();
        let mut names = BTreeMap::new();
        names.insert("run".to_string(), blake3::hash(b"run"));
        names.insert("helper".to_string(), blake3::hash(b"helper"));
        store.write_names(&names).expect("write");
        assert_eq!(store.names().expect("read"), names);
    }

    #[test]
    fn load_closure_pulls_dependencies() {
        let (_dir, store) = temp_store();

        let dep = plain(1.0);
        let dep_hash = store.put_object(&dep).expect("put dep");
        let root = StoredObject::Plain(ObjectFunction {
            bytecode: vec![9],
            constants: vec![ObjectConstant::Ref(ObjectRef::External(dep_hash))],
            local_count: 0,
            param_count: 0,
            dependencies: vec![ObjectRef::External(dep_hash)],
        });
        let root_hash = store.put_object(&root).expect("put root");

        let mut memory = Store::new();
        let loaded = store
            .load_closure(&root_hash, &mut memory)
            .expect("load closure");
        assert_eq!(loaded, 2);
        assert!(memory.contains(&root_hash));
        assert!(memory.contains(&dep_hash));
    }

    #[test]
    fn load_closure_fails_on_missing_dependency() {
        let (_dir, store) = temp_store();

        let ghost = blake3::hash(b"ghost");
        let root = StoredObject::Plain(ObjectFunction {
            bytecode: vec![9],
            constants: vec![],
            local_count: 0,
            param_count: 0,
            dependencies: vec![ObjectRef::External(ghost)],
        });
        let root_hash = store.put_object(&root).expect("put root");

        let mut memory = Store::new();
        assert!(store.load_closure(&root_hash, &mut memory).is_err());
    }

    #[test]
    fn verify_reports_clean_and_dangling() {
        let (_dir, store) = temp_store();
        store.put_object(&plain(1.0)).expect("put");
        let report = store.verify().expect("verify");
        assert!(report.is_clean());
        assert_eq!(report.valid, 1);

        // Add an object referencing a hash nobody provides.
        let root = StoredObject::Plain(ObjectFunction {
            bytecode: vec![9],
            constants: vec![],
            local_count: 0,
            param_count: 0,
            dependencies: vec![ObjectRef::External(blake3::hash(b"ghost"))],
        });
        store.put_object(&root).expect("put");
        let report = store.verify().expect("verify");
        assert_eq!(report.dangling.len(), 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn gc_keeps_named_closure_and_removes_garbage() {
        let (_dir, store) = temp_store();

        let dep_hash = store.put_object(&plain(1.0)).expect("put dep");
        let root = StoredObject::Plain(ObjectFunction {
            bytecode: vec![9],
            constants: vec![ObjectConstant::Ref(ObjectRef::External(dep_hash))],
            local_count: 0,
            param_count: 0,
            dependencies: vec![ObjectRef::External(dep_hash)],
        });
        let root_hash = store.put_object(&root).expect("put root");
        let garbage_hash = store.put_object(&plain(999.0)).expect("put garbage");

        let mut names = BTreeMap::new();
        names.insert("run".to_string(), root_hash);
        store.write_names(&names).expect("write names");

        let removed = store.gc(&[]).expect("gc");
        assert_eq!(removed, 1);
        assert!(store.contains(&root_hash));
        assert!(store.contains(&dep_hash));
        assert!(!store.contains(&garbage_hash));
    }

    #[test]
    fn reopening_preserves_contents() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("store");
        let hash = {
            let store = DiskStore::open(&path).expect("open");
            store.put_object(&plain(7.0)).expect("put")
        };
        let store = DiskStore::open(&path).expect("reopen");
        assert!(store.contains(&hash));
        assert!(store.get_function(&hash).expect("get").is_some());
    }

    #[test]
    fn foreign_format_is_rejected() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("store");
        std::fs::create_dir_all(&path).expect("mkdir");
        std::fs::write(path.join("format"), "something-else\n").expect("write");
        assert!(matches!(
            DiskStore::open(&path),
            Err(DiskStoreError::UnknownFormat(_))
        ));
    }
}
