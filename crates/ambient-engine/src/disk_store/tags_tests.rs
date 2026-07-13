//! Snapshot-tag unit tests: round-trip, name validation, gc protection of a
//! tagged (non-current) snapshot, and `verify` flagging a dangling tag.

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

/// A one-module manifest naming `object` as a produced object.
fn manifest_naming(package: &str, object: blake3::Hash) -> BuildManifest {
    BuildManifest {
        version: MANIFEST_VERSION,
        package_name: package.to_string(),
        dispatch_surface_hash: [0u8; 32],
        natives_contract_hash: [0u8; 32],
        core_cache_key: [0u8; 32],
        modules: vec![ManifestModule {
            module: "workspace::t::main".to_string(),
            resolved_ast_hash: [1u8; 32],
            interface_hash: [2u8; 32],
            deps: vec![],
            objects: vec![*object.as_bytes()],
            names: vec![],
            signatures: vec![],
            cache_key: [0u8; 32],
            consumed_links: vec![],
            migrations: vec![],
            lambda_parents: vec![],
            entry_point: None,
            source_path: "main.ab".to_string(),
            items: vec![],
            prelink: None,
        }],
    }
}

#[test]
fn tag_roundtrips_and_lists() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let manifest = manifest_naming("demo", a);
    let hash = store.write_snapshot(&manifest).expect("snapshot");

    store.write_tag("v1.0", &hash).expect("tag");
    store.write_tag("release-candidate", &hash).expect("tag");

    assert_eq!(store.read_tag("v1.0").expect("read"), Some(hash));
    assert_eq!(store.read_tag("missing").expect("read"), None);

    let tags = store.list_tags().expect("list");
    assert_eq!(
        tags,
        vec![
            ("release-candidate".to_string(), hash),
            ("v1.0".to_string(), hash),
        ]
    );
}

#[test]
fn invalid_tag_names_are_rejected() {
    let (_dir, store) = temp_store();
    let hash = blake3::hash(b"m");
    for bad in ["", ".", "..", "a/b", "a b", "a:b", "a\\b", "tag*"] {
        assert!(
            !is_valid_tag_name(bad),
            "{bad:?} should be an invalid tag name"
        );
        assert!(
            matches!(
                store.write_tag(bad, &hash),
                Err(DiskStoreError::InvalidTagName(_))
            ),
            "{bad:?} should be rejected by write_tag"
        );
    }
    for good in ["v1", "v1.2.3", "release_candidate", "a-b.c_d"] {
        assert!(is_valid_tag_name(good), "{good:?} should be valid");
    }
}

#[test]
fn gc_keeps_a_tagged_snapshot_even_when_superseded() {
    let (_dir, store) = temp_store();
    let a = store.put_object(&plain(1.0)).expect("put");
    let b = store.put_object(&plain(2.0)).expect("put");

    // Snapshot 1 names `a`; tag it. Snapshot 2 (current) names `b`.
    let m1 = manifest_naming("first", a);
    let h1 = store.write_snapshot(&m1).expect("snapshot 1");
    store.write_tag("keep", &h1).expect("tag");
    let m2 = manifest_naming("second", b);
    let h2 = store.write_snapshot(&m2).expect("snapshot 2");
    assert_ne!(h1, h2);

    let removed = store.gc(&[]).expect("gc");

    // The tag protects snapshot 1's object AND its manifest, even though the
    // pointer moved on.
    assert!(store.contains(&a), "tagged snapshot object survives gc");
    assert!(store.contains(&b), "current snapshot object survives gc");
    assert!(store.meta_path(&h1).exists(), "tagged manifest survives gc");
    assert!(
        store.meta_path(&h2).exists(),
        "current manifest survives gc"
    );
    // Tag still resolves.
    assert_eq!(store.read_tag("keep").expect("read"), Some(h1));
    let _ = removed;
}

#[test]
fn verify_flags_a_dangling_tag() {
    let (_dir, store) = temp_store();
    // A tag naming a manifest that was never written.
    let phantom = blake3::hash(b"never written");
    store.write_tag("broken", &phantom).expect("tag");

    let report = store.verify().expect("verify");
    assert!(!report.is_clean(), "dangling tag makes the store unclean");
    assert!(
        report.bad_tags.iter().any(|(name, _)| name == "broken"),
        "verify flags the dangling tag: {:?}",
        report.bad_tags
    );
}

#[test]
fn verify_flags_a_malformed_tag_file() {
    let (_dir, store) = temp_store();
    let tags = store.root().join("tags");
    std::fs::create_dir_all(&tags).expect("mkdir");
    std::fs::write(tags.join("garbage"), b"not a tag\n").expect("write");

    let report = store.verify().expect("verify");
    assert!(
        report.bad_tags.iter().any(|(name, _)| name == "garbage"),
        "verify flags the malformed tag: {:?}",
        report.bad_tags
    );
}
