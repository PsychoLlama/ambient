//! The `prelink/` store area: content-addressed pre-link symbolic-form blobs
//! (`crate::compiler::PrelinkModule`), the input to the build cache's relink
//! fast path.
//!
//! ```text
//! <root>/prelink/<2hex>/<62hex>    one canonical prelink blob per file
//! ```
//!
//! A blob is `blake3`-named and self-verifying, exactly like an object: a read
//! re-hashes the bytes and rejects a mismatch. Because the store is a
//! rebuildable cache, the build read path ([`DiskStore::get_prelink`]) treats
//! every failure — missing, corrupt, undecodable, wrong version — as "no
//! prelink" (a full recompile, never an error), self-healing a bad file the
//! way object reads do. [`DiskStore::verify`] reports the same conditions
//! loudly.

use std::io;
use std::path::PathBuf;

use super::{DiskStore, DiskStoreError, write_atomic};
use crate::compiler::PrelinkModule;

impl DiskStore {
    /// Path of the pre-link blob file for a hash, under `prelink/`.
    #[must_use]
    pub fn prelink_path(&self, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root()
            .join("prelink")
            .join(&hex.as_str()[..2])
            .join(&hex.as_str()[2..])
    }

    /// Write a pre-link blob at its content hash, returning that hash. Blobs
    /// are immutable and content-addressed, so an identical file already on
    /// disk is left in place.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn put_prelink(&self, bytes: &[u8]) -> Result<blake3::Hash, DiskStoreError> {
        let hash = blake3::hash(bytes);
        let path = self.prelink_path(&hash);
        if !path.exists() {
            write_atomic(self.root(), &path, bytes)?;
        }
        Ok(hash)
    }

    /// Load and hash-verify the pre-link blob at `hash`, decoded into a
    /// [`PrelinkModule`]. Every failure — missing, hash mismatch, or undecodable
    /// (including an unknown version) — yields `None`: the caller recompiles.
    /// A corrupt or undecodable file is deleted so the next persist rewrites
    /// the correct bytes, mirroring the object store's self-heal.
    ///
    /// # Errors
    ///
    /// Fails only on unexpected I/O (a permission error, say); a missing or
    /// corrupt blob is `Ok(None)`.
    pub fn get_prelink(
        &self,
        hash: &blake3::Hash,
    ) -> Result<Option<PrelinkModule>, DiskStoreError> {
        let path = self.prelink_path(hash);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if blake3::hash(&bytes) != *hash {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        if let Ok(module) = PrelinkModule::decode(&bytes) {
            Ok(Some(module))
        } else {
            // Undecodable (including an unknown version): drop it so the next
            // persist rewrites correct bytes.
            let _ = std::fs::remove_file(&path);
            Ok(None)
        }
    }

    /// Every hash with a pre-link blob file in the store.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors. A missing `prelink/` directory reads back empty.
    pub fn all_prelink_hashes(&self) -> Result<Vec<blake3::Hash>, DiskStoreError> {
        let mut hashes = Vec::new();
        let dir = self.root().join("prelink");
        let entries = match std::fs::read_dir(&dir) {
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
