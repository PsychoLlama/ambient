//! Disk store unit tests.

use super::*;
use crate::compiler::MigrationRecord;
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
fn load_closure_pulls_in_const_value_objects() {
    use ambient_ability::Value;

    let (_dir, store) = temp_store();

    // A `const` value object (a leaf) and a function that depends on it.
    let value_object = crate::object::value_object(&Value::Number(42.0)).expect("value object");
    let value_hash = store.put_object(&value_object).expect("put value");

    let root = StoredObject::Plain(ObjectFunction {
        bytecode: vec![9],
        // The reference is a value ref in the pool plus a dependency edge.
        constants: vec![ObjectConstant::ValueRef(value_hash)],
        local_count: 0,
        param_count: 0,
        dependencies: vec![ObjectRef::External(value_hash)],
    });
    let root_hash = store.put_object(&root).expect("put root");

    let mut memory = Store::new();
    store
        .load_closure(&root_hash, &mut memory)
        .expect("load closure");
    // The function loaded, and its const value object came along as a leaf.
    assert!(memory.contains(&root_hash));
    assert_eq!(memory.get_value(&value_hash), Some(&Value::Number(42.0)));
}

#[test]
fn identical_const_values_deduplicate_on_disk() {
    use ambient_ability::Value;

    let (_dir, store) = temp_store();
    let a = crate::object::value_object(&Value::string("blob")).expect("value object");
    let b = crate::object::value_object(&Value::string("blob")).expect("value object");

    let first = store.put_object(&a).expect("put a");
    // Byte-identical content ⇒ same path ⇒ the second write is a no-op.
    assert!(!store.put_object_at(&b.hash(), &b).expect("put b"));
    assert_eq!(first, b.hash());
}

#[test]
fn put_module_binds_const_names_in_index() {
    use ambient_ability::Value;

    let (_dir, store) = temp_store();

    // A module carrying one named const value object (no functions).
    let mut module = CompiledModule::new();
    let value_object = crate::object::value_object(&Value::Number(7.0)).expect("value object");
    let hash = value_object.hash();
    module.objects.insert(hash, value_object);
    module
        .const_names
        .insert(std::sync::Arc::from("SEVEN"), hash);

    store.put_module(&module).expect("put module");

    // The const is a first-class named binding, resolving to a Value object.
    let names = store.names().expect("names");
    assert_eq!(names.get("SEVEN"), Some(&hash));
    assert!(matches!(
        store.get_object(&hash).expect("get object"),
        Some(StoredObject::Value(_))
    ));
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
fn gc_keeps_a_bare_constant_pool_ref() {
    // A function passed *as a value* (or a dictionary tuple entry)
    // is a `PushConst` FunctionRef with no `dependencies` entry —
    // the reachability walk must read the constant pool too, or gc
    // purges live code.
    let (_dir, store) = temp_store();

    let held_hash = store.put_object(&plain(2.0)).expect("put held");
    let root = StoredObject::Plain(ObjectFunction {
        bytecode: vec![9],
        constants: vec![ObjectConstant::Ref(ObjectRef::External(held_hash))],
        local_count: 0,
        param_count: 0,
        dependencies: vec![],
    });
    let root_hash = store.put_object(&root).expect("put root");

    let mut names = BTreeMap::new();
    names.insert("run".to_string(), root_hash);
    store.write_names(&names).expect("write names");

    let removed = store.gc(&[]).expect("gc");
    assert_eq!(removed, 0, "the constant-pool ref is reachable");
    assert!(store.contains(&held_hash));
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

// ─────────────────────────────────────────────────────────────────────
// Signatures and migrations (pack v2 parity)
// ─────────────────────────────────────────────────────────────────────

/// A module binding one function name to `hash`, optionally with a
/// rendered signature.
fn module_binding(name: &str, hash: blake3::Hash, signature: Option<&str>) -> CompiledModule {
    let mut module = CompiledModule::new();
    module
        .function_names
        .insert(std::sync::Arc::from(name), hash);
    if let Some(sig) = signature {
        module
            .signatures
            .insert(std::sync::Arc::from(name), std::sync::Arc::from(sig));
    }
    module
}

#[test]
fn signatures_roundtrip_through_disk() {
    let (_dir, store) = temp_store();

    // Signatures contain spaces (arrows, `with` rows, `where` clauses).
    let hash = blake3::hash(b"gcd");
    let mut module = module_binding("gcd", hash, Some("(Number, Number) -> Number"));
    let other = blake3::hash(b"greet");
    module
        .function_names
        .insert(std::sync::Arc::from("greet"), other);
    module.signatures.insert(
        std::sync::Arc::from("greet"),
        std::sync::Arc::from("(String) -> String with core::io::Console"),
    );

    store.put_module(&module).expect("put module");

    let sigs = store.signatures().expect("read signatures");
    assert_eq!(
        sigs.get("gcd").map(String::as_str),
        Some("(Number, Number) -> Number")
    );
    assert_eq!(
        sigs.get("greet").map(String::as_str),
        Some("(String) -> String with core::io::Console")
    );

    // The convenience view pairs each name with its (hash, signature).
    let bindings = store.name_bindings().expect("read bindings");
    assert_eq!(
        bindings.get("gcd"),
        Some(&(hash, Some("(Number, Number) -> Number".to_string())))
    );
    assert_eq!(bindings.get("greet").map(|(h, _)| *h), Some(other));
}

#[test]
fn signatures_reload_after_reopen() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store");
    let hash = blake3::hash(b"f");
    {
        let store = DiskStore::open(&path).expect("open");
        let module = module_binding("f", hash, Some("() -> ()"));
        store.put_module(&module).expect("put");
    }
    let store = DiskStore::open(&path).expect("reopen");
    assert_eq!(
        store
            .signatures()
            .expect("read")
            .get("f")
            .map(String::as_str),
        Some("() -> ()")
    );
}

#[test]
fn rebinding_without_signature_drops_the_stale_one() {
    let (_dir, store) = temp_store();

    let old = blake3::hash(b"v1");
    store
        .put_module(&module_binding("f", old, Some("() -> Number")))
        .expect("put v1");
    assert!(store.signatures().expect("read").contains_key("f"));

    // Rebind `f` to a new hash but ship no rendered signature. A stale
    // rendering could wrongly *rebind* under the deploy rule, so it must
    // be dropped, not kept.
    let new = blake3::hash(b"v2");
    store
        .put_module(&module_binding("f", new, None))
        .expect("put v2");

    let sigs = store.signatures().expect("read");
    assert!(
        !sigs.contains_key("f"),
        "a name re-bound without a signature must drop the stale one: {sigs:?}"
    );
    // The name binding itself still moved forward.
    assert_eq!(store.names().expect("names").get("f"), Some(&new));
}

#[test]
fn signatures_of_untouched_names_survive_a_put() {
    let (_dir, store) = temp_store();

    store
        .put_module(&module_binding("a", blake3::hash(b"a"), Some("() -> A")))
        .expect("put a");
    // A later build that only binds `b` must not disturb `a`'s signature.
    store
        .put_module(&module_binding("b", blake3::hash(b"b"), Some("() -> B")))
        .expect("put b");

    let sigs = store.signatures().expect("read");
    assert_eq!(sigs.get("a").map(String::as_str), Some("() -> A"));
    assert_eq!(sigs.get("b").map(String::as_str), Some("() -> B"));
}

#[test]
fn migrations_replace_wholesale_on_each_put() {
    let (_dir, store) = temp_store();

    let mut first = CompiledModule::new();
    first.migrations.push(MigrationRecord {
        cell: std::sync::Arc::from("app::stats"),
        old: std::sync::Arc::from("StatsV1"),
        new: std::sync::Arc::from("StatsV2"),
    });
    first.migrations.push(MigrationRecord {
        cell: std::sync::Arc::from("app::conns"),
        old: std::sync::Arc::from("ConnV1"),
        new: std::sync::Arc::from("ConnV2"),
    });
    store.put_module(&first).expect("put first");
    assert_eq!(store.migrations().expect("read").len(), 2);

    // A later complete build replaces the obligation set wholesale — they
    // are per-build facts, not accumulating per-name bindings.
    let mut second = CompiledModule::new();
    second.migrations.push(MigrationRecord {
        cell: std::sync::Arc::from("app::stats"),
        old: std::sync::Arc::from("StatsV2"),
        new: std::sync::Arc::from("StatsV3"),
    });
    store.put_module(&second).expect("put second");

    let migrations = store.migrations().expect("read");
    assert_eq!(migrations, second.migrations);
}

#[test]
fn migrations_with_whitespace_fields_roundtrip() {
    let (_dir, store) = temp_store();

    // Cell names are user string literals and fingerprints are rendered
    // types: both can carry spaces (and, in principle, other control
    // characters), so the encoding must not be whitespace-delimited.
    let mut module = CompiledModule::new();
    module.migrations.push(MigrationRecord {
        cell: std::sync::Arc::from("my cell\twith weird\tchars"),
        old: std::sync::Arc::from("(Number) -> String with core::io::Console"),
        new: std::sync::Arc::from("(Number) -> Result<String, E>"),
    });
    store.put_module(&module).expect("put");

    assert_eq!(store.migrations().expect("read"), module.migrations);
}

#[test]
fn signature_with_control_chars_roundtrips() {
    let (_dir, store) = temp_store();
    // Defensive: even if a rendering ever grew a tab or backslash, the
    // escape scheme must round-trip it exactly.
    let hash = blake3::hash(b"weird");
    let module = module_binding("weird", hash, Some("a\tb\\c end"));
    store.put_module(&module).expect("put");
    assert_eq!(
        store
            .signatures()
            .expect("read")
            .get("weird")
            .map(String::as_str),
        Some("a\tb\\c end")
    );
}

#[test]
fn old_store_without_sidecars_reads_empty() {
    // A store written before signatures/migrations existed simply lacks
    // the sidecar files. Reads must fail safe to empty, never error.
    let (_dir, store) = temp_store();
    store.put_object(&plain(1.0)).expect("put");

    assert!(store.signatures().expect("read").is_empty());
    assert!(store.migrations().expect("read").is_empty());
    assert!(store.name_bindings().expect("read").is_empty());
}
