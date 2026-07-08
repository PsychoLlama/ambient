use super::*;
use crate::bytecode::BytecodeBuilder;
use crate::bytecode::Opcode;
use crate::value::Value;

/// Create a function with a predictable hash for testing.
/// The hash is overridden to allow testing without content-address coupling.
fn make_test_function(name: &str, return_value: f64) -> CompiledFunction {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(return_value));
    builder.emit(Opcode::Return);
    let mut func = builder.build(0, 0);
    // Give it a predictable hash based on name for testing
    func.hash = blake3::hash(name.as_bytes());
    func
}

/// Create a canonical plain object for serialization testing.
fn make_plain_object(return_value: f64) -> StoredObject {
    use crate::object::{ObjectConstant, ObjectFunction};
    StoredObject::Plain(ObjectFunction {
        bytecode: vec![1, 2, 3],
        constants: vec![ObjectConstant::Number(return_value)],
        local_count: 0,
        param_count: 0,
        dependencies: vec![],
    })
}

#[test]
fn test_store_add_and_get() {
    let mut store = Store::new();
    let func = make_test_function("test::foo", 42.0);
    let hash = func.hash;

    store.add(func);

    assert!(store.contains(&hash));
    assert!(store.get(&hash).is_some());
    assert_eq!(store.len(), 1);
}

#[test]
fn test_store_missing_dependencies() {
    let mut store = Store::new();

    // Create a function with dependencies
    let dep_hash = blake3::hash(b"test::dependency");
    let mut func = make_test_function("test::caller", 1.0);
    func.dependencies = vec![dep_hash];
    let caller_hash = func.hash;

    store.add(func);

    // Dependency is missing
    let missing = store.missing_dependencies(&caller_hash);
    assert_eq!(missing, vec![dep_hash]);

    // Add the dependency
    let mut dep_func = make_test_function("test::dep_impl", 2.0);
    dep_func.hash = dep_hash;
    store.add(dep_func);

    // No longer missing
    let missing = store.missing_dependencies(&caller_hash);
    assert!(missing.is_empty());
}

#[test]
fn test_store_transitive_dependencies() {
    let mut store = Store::new();

    // Create chain: A -> B -> C
    let hash_c = blake3::hash(b"test::c");
    let hash_b = blake3::hash(b"test::b");
    let hash_a = blake3::hash(b"test::a");

    let mut func_c = make_test_function("c_impl", 1.0);
    func_c.hash = hash_c;
    func_c.dependencies = vec![];

    let mut func_b = make_test_function("b_impl", 2.0);
    func_b.hash = hash_b;
    func_b.dependencies = vec![hash_c];

    let mut func_a = make_test_function("a_impl", 3.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_b];

    store.add(func_c);
    store.add(func_b);
    store.add(func_a);

    let deps = store.transitive_dependencies(&hash_a);
    assert!(deps.contains(&hash_b));
    assert!(deps.contains(&hash_c));
}

#[test]
fn test_store_extract_with_dependencies() {
    let mut store = Store::new();

    let hash_dep = blake3::hash(b"test::dep");
    let hash_main = blake3::hash(b"test::main");
    let hash_other = blake3::hash(b"test::other");

    let mut func_dep = make_test_function("dep", 1.0);
    func_dep.hash = hash_dep;

    let mut func_main = make_test_function("main", 2.0);
    func_main.hash = hash_main;
    func_main.dependencies = vec![hash_dep];

    let mut func_other = make_test_function("other", 3.0);
    func_other.hash = hash_other;

    store.add(func_dep);
    store.add(func_main);
    store.add(func_other);

    // Extract main and its deps, should not include "other"
    let extracted = store.extract_with_dependencies(&hash_main);
    assert_eq!(extracted.len(), 2);
    assert!(extracted.contains(&hash_main));
    assert!(extracted.contains(&hash_dep));
    assert!(!extracted.contains(&hash_other));
}

#[test]
fn test_store_merge() {
    let mut store1 = Store::new();
    let mut store2 = Store::new();

    let func1 = make_test_function("test::a", 1.0);
    let hash1 = func1.hash;
    store1.add(func1);

    let func2 = make_test_function("test::b", 2.0);
    let hash2 = func2.hash;
    store2.add(func2);

    store1.merge(&store2);

    assert_eq!(store1.len(), 2);
    assert!(store1.contains(&hash1));
    assert!(store1.contains(&hash2));
}

#[test]
fn test_store_serialize_roundtrip() {
    let mut store = Store::new();

    let object = make_plain_object(42.0);
    let hash = store.add_object(object).expect("add_object failed");

    // Serialize
    let data = store.serialize().expect("serialization failed");

    // Deserialize
    let store2 = Store::deserialize(&data).expect("deserialization failed");

    assert_eq!(store2.len(), 1);
    assert!(store2.contains(&hash));

    let func2 = store2.get(&hash).expect("function not found");
    assert_eq!(func2.constants.len(), 1);
    assert_eq!(func2.hash, hash);
}

#[test]
fn test_store_serialize_with_dependencies() {
    use crate::object::{ObjectConstant, ObjectFunction, ObjectRef};

    let mut store = Store::new();

    let dep_object = make_plain_object(1.0);
    let dep_hash = store.add_object(dep_object).expect("add dep failed");

    let main_object = StoredObject::Plain(ObjectFunction {
        bytecode: vec![9, 9],
        constants: vec![ObjectConstant::Ref(ObjectRef::External(dep_hash))],
        local_count: 0,
        param_count: 0,
        dependencies: vec![ObjectRef::External(dep_hash)],
    });
    let main_hash = store.add_object(main_object).expect("add main failed");

    // Serialize and deserialize
    let data = store.serialize().expect("serialization failed");
    let store2 = Store::deserialize(&data).expect("deserialization failed");

    assert_eq!(store2.len(), 2);
    assert!(store2.contains(&dep_hash));
    assert!(store2.contains(&main_hash));

    let main2 = store2.get(&main_hash).expect("main function not found");
    assert_eq!(main2.dependencies, vec![dep_hash]);
}

#[test]
fn test_recursive_group_survives_roundtrip() {
    use crate::object::{GroupMember, ObjectConstant, ObjectFunction, ObjectRef, member_hash};

    // Mutually recursive pair, stored as one group object.
    let make_member = |name: &str, other: u32| GroupMember {
        name: Some(name.to_string()),
        function: ObjectFunction {
            bytecode: vec![7],
            constants: vec![ObjectConstant::Ref(ObjectRef::Internal(other))],
            local_count: 0,
            param_count: 1,
            dependencies: vec![ObjectRef::Internal(other)],
        },
    };
    let group = StoredObject::Group(vec![make_member("even", 1), make_member("odd", 0)]);
    let group_hash = group.hash();

    let mut store = Store::new();
    store.add_object(group).expect("add group failed");

    let even_hash = member_hash(&group_hash, 0, 2);
    let odd_hash = member_hash(&group_hash, 1, 2);
    assert!(store.contains(&even_hash));
    assert!(store.contains(&odd_hash));

    // The old JSON format failed hash verification for recursive
    // functions (member hashes are not recomputable from a single
    // function). The pack format ships the group object, so recursion
    // survives serialization.
    let data = store.extract_pack(&even_hash).expect("extract failed");
    let store2 = Store::deserialize(&data).expect("deserialize failed");
    assert!(store2.contains(&even_hash));
    assert!(store2.contains(&odd_hash));
    assert_eq!(
        store2.get(&even_hash).expect("even missing").dependencies,
        vec![odd_hash]
    );
}

#[test]
fn test_extract_pack_rejects_unportable_function() {
    let mut store = Store::new();
    let func = make_test_function("test::scratch", 1.0);
    let hash = func.hash;
    store.add(func);

    assert!(matches!(
        store.extract_pack(&hash),
        Err(StoreError::MissingObject(h)) if h == hash
    ));
}

// =========================================================================
// SCC Detection Tests
// =========================================================================

#[test]
fn test_scc_single_function() {
    let mut store = Store::new();
    let func = make_test_function("test::single", 42.0);
    let hash = func.hash;
    store.add(func);

    let analysis = store.compute_sccs();
    assert_eq!(analysis.components.len(), 1);
    assert!(analysis.components[0].is_singleton());
    assert_eq!(analysis.components[0].members, vec![hash]);
}

#[test]
fn test_scc_linear_chain() {
    let mut store = Store::new();

    // Create A -> B -> C (no cycles)
    let hash_c = blake3::hash(b"test::c");
    let hash_b = blake3::hash(b"test::b");
    let hash_a = blake3::hash(b"test::a");

    let mut func_c = make_test_function("c", 1.0);
    func_c.hash = hash_c;
    func_c.dependencies = vec![];

    let mut func_b = make_test_function("b", 2.0);
    func_b.hash = hash_b;
    func_b.dependencies = vec![hash_c];

    let mut func_a = make_test_function("a", 3.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_b];

    store.add(func_c);
    store.add(func_b);
    store.add(func_a);

    let analysis = store.compute_sccs();
    // Each function is its own SCC (no cycles)
    assert_eq!(analysis.components.len(), 3);
    for scc in &analysis.components {
        assert!(scc.is_singleton());
    }
    assert!(!analysis.is_recursive(&hash_a));
    assert!(!analysis.is_recursive(&hash_b));
    assert!(!analysis.is_recursive(&hash_c));
}

#[test]
fn test_scc_mutual_recursion() {
    let mut store = Store::new();

    // Create A <-> B (mutual recursion)
    let hash_a = blake3::hash(b"test::a");
    let hash_b = blake3::hash(b"test::b");

    let mut func_a = make_test_function("a", 1.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_b];

    let mut func_b = make_test_function("b", 2.0);
    func_b.hash = hash_b;
    func_b.dependencies = vec![hash_a];

    store.add(func_a);
    store.add(func_b);

    let analysis = store.compute_sccs();
    // Both functions are in the same SCC
    assert_eq!(analysis.components.len(), 1);
    assert_eq!(analysis.components[0].members.len(), 2);
    assert!(analysis.components[0].members.contains(&hash_a));
    assert!(analysis.components[0].members.contains(&hash_b));
    assert!(analysis.is_recursive(&hash_a));
    assert!(analysis.is_recursive(&hash_b));
}

#[test]
fn test_scc_self_recursion() {
    let mut store = Store::new();

    // Create A -> A (self recursion)
    let hash_a = blake3::hash(b"test::a");

    let mut func_a = make_test_function("a", 1.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_a]; // Calls itself

    store.add(func_a);

    let analysis = store.compute_sccs();
    // Self-recursive function is its own SCC but is_recursive returns false
    // because it's a singleton (self-recursion is handled separately)
    assert_eq!(analysis.components.len(), 1);
    assert!(analysis.components[0].is_singleton());
}

#[test]
fn test_scc_complex_graph() {
    let mut store = Store::new();

    // Create a graph with multiple SCCs:
    //   A -> B -> C -> D
    //        ^    |
    //        +----+
    //   E (standalone)
    let hash_a = blake3::hash(b"test::a");
    let hash_b = blake3::hash(b"test::b");
    let hash_c = blake3::hash(b"test::c");
    let hash_d = blake3::hash(b"test::d");
    let hash_e = blake3::hash(b"test::e");

    let mut func_a = make_test_function("a", 1.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_b];

    let mut func_b = make_test_function("b", 2.0);
    func_b.hash = hash_b;
    func_b.dependencies = vec![hash_c];

    let mut func_c = make_test_function("c", 3.0);
    func_c.hash = hash_c;
    func_c.dependencies = vec![hash_b, hash_d]; // C -> B creates cycle, C -> D

    let mut func_d = make_test_function("d", 4.0);
    func_d.hash = hash_d;
    func_d.dependencies = vec![];

    let mut func_e = make_test_function("e", 5.0);
    func_e.hash = hash_e;
    func_e.dependencies = vec![];

    store.add(func_a);
    store.add(func_b);
    store.add(func_c);
    store.add(func_d);
    store.add(func_e);

    let analysis = store.compute_sccs();

    // B and C form an SCC (mutual recursion)
    // A, D, E are each their own SCC
    assert_eq!(analysis.components.len(), 4);

    // B and C should be in the same SCC
    let bc_scc = analysis.scc_for(&hash_b).expect("B should have an SCC");
    assert_eq!(bc_scc.members.len(), 2);
    assert!(bc_scc.members.contains(&hash_b));
    assert!(bc_scc.members.contains(&hash_c));

    // A, D, E should each be singletons
    assert!(
        analysis
            .scc_for(&hash_a)
            .is_some_and(GenericScc::is_singleton)
    );
    assert!(
        analysis
            .scc_for(&hash_d)
            .is_some_and(GenericScc::is_singleton)
    );
    assert!(
        analysis
            .scc_for(&hash_e)
            .is_some_and(GenericScc::is_singleton)
    );
}

#[test]
fn test_mutual_recursion_group() {
    let mut store = Store::new();

    let hash_a = blake3::hash(b"test::a");
    let hash_b = blake3::hash(b"test::b");

    let mut func_a = make_test_function("a", 1.0);
    func_a.hash = hash_a;
    func_a.dependencies = vec![hash_b];

    let mut func_b = make_test_function("b", 2.0);
    func_b.hash = hash_b;
    func_b.dependencies = vec![hash_a];

    store.add(func_a);
    store.add(func_b);

    let group = store.mutual_recursion_group(&hash_a);
    assert!(group.is_some());
    let group = group.expect("group should exist");
    assert_eq!(group.len(), 2);
    assert!(group.contains(&hash_a));
    assert!(group.contains(&hash_b));
}
