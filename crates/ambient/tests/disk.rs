//! The store on disk. See `spec/build.md`.
//!
//! These tests write under the system's temporary directory, and remove
//! what they wrote.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ambient::builder::{self as b, ty};
use ambient::engine::{Engine, Stage};
use ambient::store::{DiskStore, Store};
use ambient::workspace::Workspace;
use support::{Recording, value, workspace};

/// A directory of this test's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("ambient-store-{}-{unique}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn program() -> Workspace {
    let main = b::function("main").returns(ty::int()).body(b::int(1));
    workspace([("app", "main", b::module().define(main))])
}

/// Compiles `main` over the store in `directory`, and says which checks
/// ran.
fn build(directory: &Path) -> Vec<String> {
    let workspace = program();
    let store = DiskStore::open(directory).unwrap();
    let engine = Engine::new(&workspace, &store);
    engine
        .compile(&value("main", "main"), &Recording::default())
        .unwrap();
    engine.ran(Stage::Check)
}

/// Every file under `directory` that holds an entry.
fn entries(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .flat_map(|shard| std::fs::read_dir(shard.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[test]
fn what_one_process_stored_the_next_finds() {
    let scratch = Scratch::new();

    assert_eq!(build(&scratch.0), ["app:main.main"]);
    assert!(build(&scratch.0).is_empty());
}

#[test]
fn an_entry_that_is_not_what_was_written_is_computed_again() {
    let scratch = Scratch::new();
    build(&scratch.0);
    for entry in entries(&scratch.0) {
        let mut bytes = std::fs::read(&entry).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&entry, bytes).unwrap();
    }

    assert_eq!(build(&scratch.0), ["app:main.main"]);
}

#[test]
fn an_entry_is_read_back_as_it_was_written() {
    let scratch = Scratch::new();
    let store = DiskStore::open(&scratch.0).unwrap();
    let key = ambient::hash::Hasher::new("test").finish();

    assert_eq!(store.get(&key), None);
    store.put(key, vec![1, 2, 3]);
    assert_eq!(store.get(&key).as_deref(), Some(&[1, 2, 3][..]));
    store.put(key, Vec::new());
    assert_eq!(store.get(&key).as_deref(), Some(&[][..]));
}
