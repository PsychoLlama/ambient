//! The store: stage results by key. See `docs/spec/build.md`.

mod disk;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

pub use disk::DiskStore;

use crate::hash::Hash;

/// Where an engine keeps what it has computed.
///
/// Every key is derived from the content of a stage's inputs, so an entry is
/// valid for any workspace and losing one costs only time. An implementation
/// may drop entries whenever it likes.
pub trait Store {
    fn get(&self, key: &Hash) -> Option<Arc<[u8]>>;
    fn put(&self, key: Hash, value: Vec<u8>);
}

/// A store that lives and dies with the process.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<Hash, Arc<[u8]>>>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of entries held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Store for MemoryStore {
    fn get(&self, key: &Hash) -> Option<Arc<[u8]>> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    fn put(&self, key: Hash, value: Vec<u8>) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, value.into());
    }
}
