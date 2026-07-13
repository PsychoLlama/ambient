//! Snapshot tags: human names aliasing a build manifest's content hash.
//!
//! ```text
//! <root>/tags/<name>    "ambient-tag-v1 <64hex>\n"
//! ```
//!
//! A tag is a tiny, atomic, forward-compatible pointer: the same
//! temp+rename discipline as the root pointer, a version marker so the
//! format can evolve, and one file per tag so writes never contend. Tags
//! pin a manifest against garbage collection — a tagged manifest and every
//! object it transitively references survive `gc` exactly like the current
//! snapshot — so a tag is how a caller keeps a build around to diff against
//! later. `verify` flags a tag whose manifest is missing or corrupt.
//!
//! Tag names are restricted to a conservative charset (`[A-Za-z0-9._-]`, and
//! never `.`/`..`) so a name is always a safe single path component.

use std::io;
use std::path::PathBuf;

use super::{DiskStore, DiskStoreError, write_atomic};

/// Prefix (and version marker) of a tag file's single line.
const TAG_PREFIX: &str = "ambient-tag-v1 ";

/// The maximum length of a tag name — generous, but bounded so a name is
/// always a sane filename.
const MAX_TAG_NAME: usize = 128;

/// Whether `name` is an acceptable tag name: non-empty, within the length
/// bound, every character in `[A-Za-z0-9._-]`, and not the reserved `.`/`..`
/// (which are not real names and would be path-traversal).
#[must_use]
pub fn is_valid_tag_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TAG_NAME
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl DiskStore {
    /// Path of the tag file for `name`, under `tags/`.
    #[must_use]
    pub fn tag_path(&self, name: &str) -> PathBuf {
        self.root().join("tags").join(name)
    }

    /// Alias `manifest` under the human name `name`, atomically. Overwrites
    /// an existing tag of the same name (last writer wins, like the root
    /// pointer). The caller should ensure the manifest exists under `meta/`;
    /// a tag pointing at a missing manifest is later flagged by `verify`.
    ///
    /// # Errors
    ///
    /// [`DiskStoreError::InvalidTagName`] if `name` is not a valid tag name,
    /// or an I/O error.
    pub fn write_tag(&self, name: &str, manifest: &blake3::Hash) -> Result<(), DiskStoreError> {
        if !is_valid_tag_name(name) {
            return Err(DiskStoreError::InvalidTagName(name.to_string()));
        }
        let content = format!("{TAG_PREFIX}{}\n", manifest.to_hex());
        write_atomic(self.root(), &self.tag_path(name), content.as_bytes())?;
        Ok(())
    }

    /// The manifest hash a tag names, or `None` if the tag is absent or its
    /// file is malformed (bad marker / bad hex). A malformed tag reads back
    /// as absent here; `verify` reports it loudly.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O reading the tag file.
    pub fn read_tag(&self, name: &str) -> Result<Option<blake3::Hash>, DiskStoreError> {
        let content = match std::fs::read_to_string(self.tag_path(name)) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(parse_tag(&content))
    }

    /// Every tag `(name, manifest hash)`, sorted by name. A malformed tag
    /// file is skipped here (and flagged by `verify`); a file whose name is
    /// not a valid tag name is ignored.
    ///
    /// # Errors
    ///
    /// Fails on unexpected I/O. A missing `tags/` directory reads back empty.
    pub fn list_tags(&self) -> Result<Vec<(String, blake3::Hash)>, DiskStoreError> {
        let mut tags = Vec::new();
        let dir = self.root().join("tags");
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(tags),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_valid_tag_name(&name) {
                continue;
            }
            if let Some(hash) = self.read_tag(&name)? {
                tags.push((name, hash));
            }
        }
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(tags)
    }

    /// Every valid tag paired with a diagnostic, for `verify`: a tag whose
    /// file is malformed, or whose manifest is missing/corrupt/unknown, is
    /// reported with a reason. Healthy tags contribute nothing.
    ///
    /// # Errors
    ///
    /// Fails on unexpected I/O.
    pub(crate) fn tag_health(&self) -> Result<Vec<(String, String)>, DiskStoreError> {
        let mut bad = Vec::new();
        let dir = self.root().join("tags");
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(bad),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_valid_tag_name(&name) {
                bad.push((name, "tag name is not a valid tag name".to_string()));
                continue;
            }
            let content = std::fs::read_to_string(entry.path())?;
            let Some(hash) = parse_tag(&content) else {
                bad.push((name, "tag file is malformed".to_string()));
                continue;
            };
            match self.load_manifest(&hash) {
                Ok(Some(_)) => {}
                Ok(None) => bad.push((name, format!("names missing manifest {hash}"))),
                Err(e) => bad.push((name, format!("names bad manifest {hash}: {e}"))),
            }
        }
        bad.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(bad)
    }

    /// The manifest hashes every valid tag names (deduplicated is the
    /// caller's job). Feeds the gc-root computation so a tagged snapshot's
    /// objects and manifest survive collection.
    ///
    /// # Errors
    ///
    /// Fails on unexpected I/O.
    pub(crate) fn tagged_manifest_hashes(&self) -> Result<Vec<blake3::Hash>, DiskStoreError> {
        Ok(self
            .list_tags()?
            .into_iter()
            .map(|(_, hash)| hash)
            .collect())
    }
}

/// Parse a tag file's content into a manifest hash. Returns `None` for
/// anything malformed (missing marker, bad hex).
fn parse_tag(content: &str) -> Option<blake3::Hash> {
    let line = content.lines().next()?;
    let hex = line.strip_prefix(TAG_PREFIX)?.trim();
    blake3::Hash::from_hex(hex).ok()
}
