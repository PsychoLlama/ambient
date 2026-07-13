//! Build-snapshot unit tests: manifest encoding, crash-safe write/load,
//! corruption fallback vs. loud `verify`, and gc protection.

use super::snapshot::{BuildManifest, MANIFEST_VERSION, ManifestModule};
use super::*;
use crate::object::{ObjectConstant, ObjectFunction};

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

/// A representative manifest whose two modules reference `objects` (the first
/// as a produced object, the second as a name binding).
fn sample_manifest(objects: &[blake3::Hash]) -> BuildManifest {
    let h = |i: usize| *objects[i].as_bytes();
    BuildManifest {
        version: MANIFEST_VERSION,
        package_name: "demo".to_string(),
        dispatch_surface_hash: [7u8; 32],
        natives_contract_hash: [9u8; 32],
        modules: vec![
            ManifestModule {
                module: "core::primitives".to_string(),
                resolved_ast_hash: [1u8; 32],
                interface_hash: [2u8; 32],
                deps: vec![],
                objects: vec![h(0)],
                names: vec![("core::primitives::sqrt".to_string(), h(0))],
                signatures: vec![(
                    "core::primitives::sqrt".to_string(),
                    "(Number) -> Number".to_string(),
                )],
            },
            ManifestModule {
                module: "workspace::demo::math".to_string(),
                resolved_ast_hash: [3u8; 32],
                interface_hash: [4u8; 32],
                deps: vec!["core::primitives".to_string()],
                objects: vec![],
                names: vec![("workspace::demo::math::gcd".to_string(), h(1))],
                signatures: vec![],
            },
        ],
    }
}

#[test]
fn manifest_roundtrips_byte_identically() {
    let manifest = sample_manifest(&[blake3::hash(b"a"), blake3::hash(b"b")]);
    let bytes = manifest.encode();
    let decoded = BuildManifest::decode(&bytes).expect("decode");
    assert_eq!(decoded, manifest);
    // encode ∘ decode ∘ encode is the identity.
    assert_eq!(decoded.encode(), bytes);
}

#[test]
fn manifest_rejects_bad_magic_version_and_trailing() {
    let manifest = sample_manifest(&[blake3::hash(b"a"), blake3::hash(b"b")]);
    let bytes = manifest.encode();

    // Truncation.
    assert!(BuildManifest::decode(&bytes[..bytes.len() - 1]).is_err());

    // Trailing bytes.
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(
        BuildManifest::decode(&extra),
        Err(snapshot::ManifestError::TrailingBytes)
    );

    // Bad magic.
    let mut bad_magic = bytes.clone();
    bad_magic[0] ^= 0xff;
    assert_eq!(
        BuildManifest::decode(&bad_magic),
        Err(snapshot::ManifestError::BadMagic)
    );

    // Unknown version.
    let mut bad_version = bytes.clone();
    bad_version[4] = MANIFEST_VERSION + 1;
    assert_eq!(
        BuildManifest::decode(&bad_version),
        Err(snapshot::ManifestError::BadVersion(MANIFEST_VERSION + 1))
    );
}

#[test]
fn write_and_load_snapshot_roundtrips() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let b = store.put_object(&plain(2.0)).expect("put");
    let manifest = sample_manifest(&[a, b]);

    let hash = store.write_snapshot(&manifest).expect("write snapshot");
    assert_eq!(store.snapshot_pointer().expect("pointer"), Some(hash));

    let loaded = store.current_snapshot().expect("load").expect("present");
    assert_eq!(loaded, manifest);
    // The snapshot is healthy.
    assert_eq!(store.snapshot_health().expect("health"), None);
}

#[test]
fn absent_snapshot_is_none() {
    let (_dir, store) = temp_store();
    assert_eq!(store.current_snapshot().expect("load"), None);
    assert_eq!(store.snapshot_pointer().expect("pointer"), None);
    assert_eq!(store.snapshot_health().expect("health"), None);
}

#[test]
fn corrupt_manifest_is_absent_in_build_path_but_loud_in_verify() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let b = store.put_object(&plain(2.0)).expect("put");
    let manifest = sample_manifest(&[a, b]);
    let hash = store.write_snapshot(&manifest).expect("write snapshot");

    // Flip a byte in the manifest file.
    let path = store.meta_path(&hash);
    let mut bytes = std::fs::read(&path).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, &bytes).expect("write");

    // Build path: silent fallback to "no snapshot".
    assert_eq!(store.current_snapshot().expect("load"), None);

    // Verify: loud.
    let report = store.verify().expect("verify");
    assert!(report.dangling_snapshot.is_some(), "{report:?}");
    assert!(!report.is_clean());
}

#[test]
fn malformed_pointer_is_absent_but_loud() {
    let (_dir, store) = temp_store();
    std::fs::write(store.root().join(snapshot::SNAPSHOT_POINTER), b"garbage\n")
        .expect("write pointer");

    assert_eq!(store.current_snapshot().expect("load"), None);
    let report = store.verify().expect("verify");
    assert!(report.dangling_snapshot.is_some(), "{report:?}");
}

#[test]
fn missing_manifest_behind_pointer_is_loud() {
    let (_dir, store) = temp_store();
    // A pointer naming a manifest that was never written.
    let phantom = blake3::hash(b"never written");
    let content = format!("ambient-snapshot-v1 {}\n", phantom.to_hex());
    std::fs::write(store.root().join(snapshot::SNAPSHOT_POINTER), content).expect("write");

    assert_eq!(store.current_snapshot().expect("load"), None);
    let report = store.verify().expect("verify");
    assert!(report.dangling_snapshot.is_some(), "{report:?}");
}

#[test]
fn gc_keeps_snapshot_reachable_objects_and_manifest() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let b = store.put_object(&plain(2.0)).expect("put");
    let garbage = store.put_object(&plain(99.0)).expect("put");

    let manifest = sample_manifest(&[a, b]);
    let manifest_hash = store.write_snapshot(&manifest).expect("write snapshot");

    // No names index, no extra roots: only the snapshot protects a, b.
    let removed = store.gc(&[]).expect("gc");
    assert!(removed >= 1, "garbage should be collected");

    assert!(store.contains(&a), "snapshot object a survives gc");
    assert!(store.contains(&b), "snapshot object b survives gc");
    assert!(!store.contains(&garbage), "unreferenced object collected");

    // The manifest is still loadable after gc.
    let loaded = store.current_snapshot().expect("load").expect("present");
    assert_eq!(loaded, manifest);
    assert!(store.meta_path(&manifest_hash).exists());
}

#[test]
fn gc_prunes_superseded_manifests() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let b = store.put_object(&plain(2.0)).expect("put");

    // First snapshot.
    let mut m1 = sample_manifest(&[a, b]);
    m1.package_name = "first".to_string();
    let h1 = store.write_snapshot(&m1).expect("write 1");

    // Second snapshot supersedes it.
    let mut m2 = sample_manifest(&[a, b]);
    m2.package_name = "second".to_string();
    let h2 = store.write_snapshot(&m2).expect("write 2");
    assert_ne!(h1, h2);
    assert_eq!(store.all_manifest_hashes().expect("list").len(), 2);

    store.gc(&[]).expect("gc");

    // Only the current manifest survives.
    let remaining = store.all_manifest_hashes().expect("list");
    assert_eq!(remaining, vec![h2]);
    assert_eq!(
        store.current_snapshot().expect("load").expect("present"),
        m2
    );
}

#[test]
fn identical_manifests_hash_identically() {
    let objects = [blake3::hash(b"x"), blake3::hash(b"y")];
    let a = sample_manifest(&objects);
    let b = sample_manifest(&objects);
    assert_eq!(a.hash(), b.hash());
}
