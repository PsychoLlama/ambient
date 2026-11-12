//! A store kept in a directory, so that what one process computed the
//! next finds. See `spec/build.md`.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::Store;
use crate::hash::{Hash, Hasher};

/// A store in a directory: one file for each entry, named by its key.
///
/// Nothing in an entry or its name refers to a path or a time, so the
/// directory may be copied, shared, or deleted. An entry that cannot be
/// read, or is not what was written, is as good as missing: the engine
/// computes it again.
pub struct DiskStore {
    root: PathBuf,
}

/// How many bytes of an entry's file are the checksum of the rest.
const CHECKSUM: usize = 32;

fn checksum(value: &[u8]) -> Hash {
    Hasher::new("store entry").bytes(value).finish()
}

impl DiskStore {
    /// The store in the directory `root`, which is made if it is missing.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be made.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Where the entry for `key` is kept. Entries are spread over
    /// directories by the start of their key, so no directory grows large.
    fn path(&self, key: &Hash) -> PathBuf {
        let name = key.to_string();
        let (directory, file) = name.split_at(2);
        self.root.join(directory).join(file)
    }

    /// Writes an entry where a reader never sees part of it: to a file of
    /// its own, which then takes the entry's name.
    fn write(&self, key: &Hash, value: &[u8]) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = self.path(key);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let partial = self
            .root
            .join(format!("partial-{}-{unique}", std::process::id()));
        let mut file = fs::File::create(&partial)?;
        file.write_all(checksum(value).as_bytes())?;
        file.write_all(value)?;
        drop(file);
        fs::rename(&partial, &path).inspect_err(|_| {
            // Nothing reads a partial file, so leaving one costs only room.
            let _ = fs::remove_file(&partial);
        })
    }
}

impl Store for DiskStore {
    fn get(&self, key: &Hash) -> Option<Arc<[u8]>> {
        let bytes = fs::read(self.path(key)).ok()?;
        let (sum, value) = bytes.split_at_checked(CHECKSUM)?;
        (sum == checksum(value).as_bytes()).then(|| Arc::from(value))
    }

    fn put(&self, key: Hash, value: Vec<u8>) {
        // A store may drop an entry, so one that cannot be written is
        // dropped: the next build computes it again.
        let _ = self.write(&key, &value);
    }
}
