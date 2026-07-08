use super::*;

fn sample_function() -> ObjectFunction {
    ObjectFunction {
        bytecode: vec![1, 2, 3, 4],
        constants: vec![
            ObjectConstant::Unit,
            ObjectConstant::Bool(true),
            ObjectConstant::Number(42.5),
            ObjectConstant::String("hello".to_string()),
            ObjectConstant::Binary(vec![0xde, 0xad]),
            ObjectConstant::Ref(ObjectRef::External(blake3::hash(b"dep"))),
            ObjectConstant::Ability(ambient_core::AbilityId::from_bytes([0xab; 32])),
        ],
        local_count: 3,
        param_count: 2,
        dependencies: vec![ObjectRef::External(blake3::hash(b"dep"))],
    }
}

#[test]
fn plain_roundtrip() {
    let object = StoredObject::Plain(sample_function());
    let encoded = object.encode();
    let decoded = StoredObject::decode(&encoded).unwrap();
    assert_eq!(object, decoded);
    assert_eq!(object.hash(), decoded.hash());
}

#[test]
fn group_roundtrip() {
    let mut even = sample_function();
    even.constants
        .push(ObjectConstant::Ref(ObjectRef::Internal(1)));
    even.dependencies.push(ObjectRef::Internal(1));
    let mut odd = sample_function();
    odd.constants
        .push(ObjectConstant::Ref(ObjectRef::Internal(0)));
    odd.dependencies.push(ObjectRef::Internal(0));

    let object = StoredObject::Group(vec![
        GroupMember {
            name: Some("even".to_string()),
            function: even,
        },
        GroupMember {
            name: Some("odd".to_string()),
            function: odd,
        },
    ]);
    let encoded = object.encode();
    let decoded = StoredObject::decode(&encoded).unwrap();
    assert_eq!(object, decoded);
}

#[test]
fn redirect_roundtrip() {
    let object = StoredObject::Redirect {
        group: blake3::hash(b"group"),
        index: 7,
    };
    let decoded = StoredObject::decode(&object.encode()).unwrap();
    assert_eq!(object, decoded);
}

#[test]
fn trailing_bytes_rejected() {
    let mut encoded = StoredObject::Plain(sample_function()).encode();
    encoded.push(0);
    assert_eq!(
        StoredObject::decode(&encoded),
        Err(ObjectError::TrailingBytes)
    );
}

#[test]
fn truncation_rejected() {
    let encoded = StoredObject::Plain(sample_function()).encode();
    for len in 0..encoded.len() {
        assert!(
            StoredObject::decode(&encoded[..len]).is_err(),
            "prefix of length {len} should not decode"
        );
    }
}

#[test]
fn lying_length_prefix_fails_without_huge_allocation() {
    // A corrupted count must produce a decode error, not an attempted
    // multi-gigabyte allocation. The encoding ends with the dependency
    // list: count u32 | (tag u8 | hash [32]). Corrupt the count's high
    // byte.
    let mut encoded = StoredObject::Plain(sample_function()).encode();
    let count_high_byte = encoded.len() - 33 - 1;
    encoded[count_high_byte] = 0xff;
    assert!(StoredObject::decode(&encoded).is_err());
}

#[test]
fn corruption_changes_hash() {
    let object = StoredObject::Plain(sample_function());
    let mut encoded = object.encode();
    let hash = blake3::hash(&encoded);
    // Flip a bytecode byte: still decodes, but hash must differ.
    let idx = encoded.len() - 40;
    encoded[idx] ^= 0xff;
    assert_ne!(blake3::hash(&encoded), hash);
}

#[test]
fn internal_ref_in_plain_rejected() {
    let mut func = sample_function();
    func.dependencies.push(ObjectRef::Internal(0));
    let encoded = StoredObject::Plain(func).encode();
    assert_eq!(
        StoredObject::decode(&encoded),
        Err(ObjectError::InternalRefInPlain)
    );
}

#[test]
fn internal_ref_out_of_range_rejected() {
    let mut func = sample_function();
    func.dependencies.push(ObjectRef::Internal(5));
    let encoded = StoredObject::Group(vec![GroupMember {
        name: Some("f".to_string()),
        function: func,
    }])
    .encode();
    assert!(matches!(
        StoredObject::decode(&encoded),
        Err(ObjectError::InternalRefOutOfRange { .. })
    ));
}

#[test]
fn plain_materialize_hash_is_self_verifying() {
    let object = StoredObject::Plain(sample_function());
    let materialized = object.materialize().unwrap();
    assert_eq!(materialized.len(), 1);
    let (hash, func) = &materialized[0];
    assert_eq!(*hash, blake3::hash(&object.encode()));
    assert_eq!(func.hash, *hash);
}

#[test]
fn group_materialize_substitutes_member_hashes() {
    let mut a = sample_function();
    a.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(1))];
    a.dependencies = vec![ObjectRef::Internal(1)];
    let mut b = sample_function();
    b.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(0))];
    b.dependencies = vec![ObjectRef::Internal(0)];

    let object = StoredObject::Group(vec![
        GroupMember {
            name: Some("a".to_string()),
            function: a,
        },
        GroupMember {
            name: Some("b".to_string()),
            function: b,
        },
    ]);
    let group = object.hash();
    let materialized = object.materialize().unwrap();
    assert_eq!(materialized.len(), 2);

    let hash_a = member_hash(&group, 0, 2);
    let hash_b = member_hash(&group, 1, 2);
    assert_eq!(materialized[0].0, hash_a);
    assert_eq!(materialized[1].0, hash_b);
    // a's references point at b and vice versa.
    assert_eq!(materialized[0].1.dependencies, vec![hash_b]);
    assert_eq!(materialized[1].1.dependencies, vec![hash_a]);
    assert!(matches!(
        materialized[0].1.constants[0],
        Value::FunctionRef(h) if h == hash_b
    ));
}

#[test]
fn singleton_group_member_hash_is_group_hash() {
    let mut f = sample_function();
    f.constants = vec![ObjectConstant::Ref(ObjectRef::Internal(0))];
    f.dependencies = vec![ObjectRef::Internal(0)];
    let object = StoredObject::Group(vec![GroupMember {
        name: Some("loop_forever".to_string()),
        function: f,
    }]);
    let group = object.hash();
    let materialized = object.materialize().unwrap();
    assert_eq!(materialized[0].0, group);
    // Self-reference resolves to its own hash.
    assert_eq!(materialized[0].1.dependencies, vec![group]);
}

#[test]
fn member_name_affects_group_hash() {
    let make = |name: &str| {
        StoredObject::Group(vec![GroupMember {
            name: Some(name.to_string()),
            function: sample_function(),
        }])
    };
    assert_ne!(make("a").hash(), make("b").hash());
}

#[test]
fn value_object_roundtrips_and_hashes_by_content() {
    // Every current const form round-trips through encode → decode.
    for value in [
        Value::Unit,
        Value::Bool(true),
        Value::Number(-1.5),
        Value::string("embedded blob"),
        Value::binary(vec![0xca, 0xfe]),
    ] {
        let object = value_object(&value).expect("value object");
        let decoded = StoredObject::decode(&object.encode()).expect("decode");
        assert_eq!(object, decoded);
        assert_eq!(object.hash(), decoded.hash());
        assert_eq!(object.as_value(), Some(value));
    }
}

#[test]
fn identical_values_share_one_hash_different_values_differ() {
    // Content addressing: the hash is a pure function of the value's type
    // and bytes — the const's name never enters it.
    let a = value_object(&Value::Number(42.0)).unwrap();
    let b = value_object(&Value::Number(42.0)).unwrap();
    assert_eq!(a.hash(), b.hash(), "same value ⇒ same hash");

    let different_value = value_object(&Value::Number(43.0)).unwrap();
    assert_ne!(a.hash(), different_value.hash());

    // Same bytes, different type tag ⇒ different hash.
    let number_zero = value_object(&Value::Number(0.0)).unwrap();
    let bool_false = value_object(&Value::Bool(false)).unwrap();
    assert_ne!(number_zero.hash(), bool_false.hash());
}

#[test]
fn value_object_materializes_no_functions() {
    let object = value_object(&Value::Number(7.0)).unwrap();
    assert!(object.materialize().unwrap().is_empty());
}

#[test]
fn native_object_roundtrips_and_hashes_by_identity() {
    let uuid = uuid::Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFE_0000_0000_0001);
    let object = StoredObject::Native {
        uuid,
        param_count: 2,
    };
    let decoded = StoredObject::decode(&object.encode()).expect("decode");
    assert_eq!(object, decoded);
    assert_eq!(object.hash(), decoded.hash());
    assert_eq!(object.as_native(), Some((uuid, 2)));
    // Natives carry no bytecode; the VM resolves the uuid instead.
    assert!(object.materialize().unwrap().is_empty());

    // The hash is a pure function of (uuid, param_count): a different
    // uuid or arity is a different object, and nothing else enters.
    let same = StoredObject::Native {
        uuid,
        param_count: 2,
    };
    assert_eq!(object.hash(), same.hash());
    let other_uuid = StoredObject::Native {
        uuid: uuid::Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFE_0000_0000_0002),
        param_count: 2,
    };
    assert_ne!(object.hash(), other_uuid.hash());
    let other_arity = StoredObject::Native {
        uuid,
        param_count: 3,
    };
    assert_ne!(object.hash(), other_arity.hash());
}

#[test]
fn nan_number_roundtrips_exactly() {
    let bits = 0x7ff8_dead_beef_0001_u64;
    let func = ObjectFunction {
        bytecode: vec![],
        constants: vec![ObjectConstant::Number(f64::from_bits(bits))],
        local_count: 0,
        param_count: 0,
        dependencies: vec![],
    };
    let object = StoredObject::Plain(func);
    let decoded = StoredObject::decode(&object.encode()).unwrap();
    let StoredObject::Plain(f) = decoded else {
        panic!("expected plain")
    };
    let ObjectConstant::Number(n) = f.constants[0] else {
        panic!("expected number")
    };
    assert_eq!(n.to_bits(), bits);
}
