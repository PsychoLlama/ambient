//! The canonical encoding of values. See `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use ambient::exchange::{Malformed, Unsendable, decode, encode, hash};
use ambient::hash::Hash;
use ambient::value::{Function, Value};
use proptest::prelude::*;

/// A function over the definition with this hash.
fn function((definition, captures): ([u8; 32], Vec<Value>)) -> Value {
    Value::Function(Function {
        definition: Some(Hash::from_bytes(definition)),
        captures,
    })
}

/// Any value that can be sent.
fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        any::<f64>().prop_map(Value::Float),
        ".*".prop_map(Value::String),
        ("[a-z]*", "[a-z]*").prop_map(|(package, tag)| Value::Tag { package, tag }),
    ];
    leaf.prop_recursive(4, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Tuple),
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::List),
            prop::collection::btree_map("[a-z]*", inner.clone(), 0..4).prop_map(Value::Record),
            (any::<[u8; 32]>(), prop::collection::vec(inner, 0..4)).prop_map(function),
        ]
    })
}

/// Whether two values are one value: floats by their bits, as the
/// language compares them.
fn same(left: &Value, right: &Value) -> bool {
    format!("{left:?}") == format!("{right:?}") && encode(left).unwrap() == encode(right).unwrap()
}

proptest! {
    #[test]
    fn a_value_decodes_to_itself(value in value()) {
        let decoded = decode(&encode(&value).unwrap()).unwrap();

        prop_assert!(same(&value, &decoded));
    }

    #[test]
    fn bytes_that_decode_are_the_encoding_of_what_they_decode_to(
        bytes in prop::collection::vec(any::<u8>(), 0..64)
    ) {
        if let Ok(value) = decode(&bytes) {
            prop_assert_eq!(encode(&value).unwrap(), bytes);
        }
    }

    #[test]
    fn an_encoding_cut_short_or_run_long_does_not_decode(value in value(), extra in any::<u8>()) {
        let mut bytes = encode(&value).unwrap();
        bytes.push(extra);
        prop_assert_eq!(decode(&bytes), Err(Malformed));
        bytes.truncate(bytes.len() - 2);
        prop_assert_eq!(decode(&bytes), Err(Malformed));
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[test]
fn the_order_a_records_fields_were_written_in_is_not_in_its_encoding() {
    let one = record([("a", Value::Int(1)), ("b", Value::Int(2))]);
    let other = record([("b", Value::Int(2)), ("a", Value::Int(1))]);

    assert_eq!(hash(&one), hash(&other));
}

#[test]
fn different_shapes_with_the_same_members_differ() {
    let members = || vec![Value::Int(1)];

    assert_ne!(
        hash(&Value::Tuple(members())),
        hash(&Value::List(members()))
    );
    assert_ne!(hash(&Value::Int(1)), hash(&Value::Float(1.0)));
    assert_ne!(hash(&Value::Null), hash(&Value::Bool(false)));
}

#[test]
fn a_record_whose_fields_are_out_of_order_does_not_decode() {
    let ordered = encode(&record([("a", Value::Null), ("b", Value::Null)])).unwrap();
    // Swap the two one-letter names.
    let swapped: Vec<u8> = ordered
        .iter()
        .map(|byte| match byte {
            b'a' => b'b',
            b'b' => b'a',
            other => *other,
        })
        .collect();

    assert!(decode(&ordered).is_ok());
    assert_eq!(decode(&swapped), Err(Malformed));
}

#[test]
fn a_value_that_holds_a_cell_or_an_abort_handler_has_no_encoding() {
    let holding = |value: Value| Value::List(vec![Value::Int(1), value]);

    assert_eq!(encode(&holding(Value::Cell)), Err(Unsendable::Cell));
    assert_eq!(encode(&holding(Value::Token)), Err(Unsendable::Abort));
}

/// A value nested deeper than any stack.
fn deep() -> Value {
    (0..200_000).fold(Value::Null, |inner, _| Value::List(vec![inner]))
}

#[test]
fn a_value_nested_deeper_than_any_stack_encodes_and_decodes() {
    let deep = deep();
    let bytes = encode(&deep).unwrap();

    assert_eq!(decode(&bytes).unwrap(), deep);
}

#[test]
fn a_value_nested_deeper_than_any_stack_is_cloned_and_compared() {
    let deep = deep();

    assert_eq!(deep.clone(), deep);
    assert_ne!(Value::List(vec![deep.clone()]), deep);
}

#[test]
fn a_value_nested_deeper_than_any_stack_crosses_into_a_program_and_back() {
    use ambient::backend::native::{Host, encode as into_program};

    let deep = deep();
    let live = ambient_runtime::amb_live();
    let word = into_program(&deep);

    assert_eq!(ambient::value::decode(word, &mut Host), deep);
    // SAFETY: the word is owned here and not used again.
    unsafe { ambient_runtime::amb_release(word) };
    assert_eq!(ambient_runtime::amb_live(), live);
}

#[test]
fn a_generic_function_has_no_encoding() {
    let generic = Value::Function(Function {
        definition: Some(Hash::from_bytes([0; 32])),
        captures: vec![Value::Types],
    });

    assert_eq!(encode(&generic), Err(Unsendable::Types));
}
