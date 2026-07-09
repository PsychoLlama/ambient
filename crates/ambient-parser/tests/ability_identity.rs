//! Nominal ability identity invariants, verified end-to-end from source.
//!
//! Abilities are nominal (`unique(<uuid>)` is the identity) and a method's
//! identity is the hash of (ability uuid, canonical signature, default
//! implementation). These tests pin the properties that scheme exists for:
//!
//! - Renaming an ability or one of its methods moves no hashes.
//! - Two same-shaped abilities with different uuids are different abilities.
//! - Changing a method's default implementation re-keys the method — every
//!   perform site's hash moves with it.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::compiler::CompiledModule;

mod common;
use common::compile;

/// The hash of the single plain (non-symbol) named function in the module.
fn caller_hash(module: &CompiledModule, name: &str) -> blake3::Hash {
    *module
        .function_names
        .get(name)
        .unwrap_or_else(|| panic!("function `{name}` not found"))
}

/// Collect name → hash for every named function in the module.
fn named_hashes(module: &CompiledModule) -> HashMap<Arc<str>, blake3::Hash> {
    module
        .function_names
        .iter()
        .map(|(name, hash)| (Arc::clone(name), *hash))
        .collect()
}

#[test]
fn renaming_an_ability_moves_no_hashes() {
    let original = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000001) ability Fortune {
          fn tell(): Number { 4 }
        }
        fn consult(): Number with Fortune {
          Fortune::tell!()
        }
        ",
    );
    let renamed = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000001) ability Oracle {
          fn tell(): Number { 4 }
        }
        fn consult(): Number with Oracle {
          Oracle::tell!()
        }
        ",
    );

    assert_eq!(
        caller_hash(&original, "consult"),
        caller_hash(&renamed, "consult"),
        "renaming an ability must not move any perform site's hash"
    );
}

#[test]
fn renaming_a_method_moves_no_hashes() {
    let original = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000002) ability Fortune {
          fn tell(): Number { 4 }
        }
        fn consult(): Number with Fortune {
          Fortune::tell!()
        }
        ",
    );
    let renamed = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000002) ability Fortune {
          fn divine(): Number { 4 }
        }
        fn consult(): Number with Fortune {
          Fortune::divine!()
        }
        ",
    );

    assert_eq!(
        caller_hash(&original, "consult"),
        caller_hash(&renamed, "consult"),
        "renaming a method must not move any perform site's hash"
    );
}

#[test]
fn same_shape_different_uuid_is_a_different_ability() {
    let source = |uuid: &str| {
        format!(
            r"
            unique({uuid}) ability Fortune {{
              fn tell(): Number {{ 4 }}
            }}
            fn consult(): Number with Fortune {{
              Fortune::tell!()
            }}
            "
        )
    };
    let a = compile(&source("AB100000-0000-0000-0000-000000000003"));
    let b = compile(&source("AB100000-0000-0000-0000-000000000004"));

    assert_ne!(
        caller_hash(&a, "consult"),
        caller_hash(&b, "consult"),
        "two same-shaped abilities with different uuids must be distinct"
    );
}

#[test]
fn changing_a_default_implementation_rekeys_the_method() {
    let source = |value: &str| {
        format!(
            r"
            unique(AB100000-0000-0000-0000-000000000005) ability Fortune {{
              fn tell(): Number {{ {value} }}
            }}
            fn consult(): Number with Fortune {{
              Fortune::tell!()
            }}
            "
        )
    };
    let a = compile(&source("4"));
    let b = compile(&source("5"));

    assert_ne!(
        caller_hash(&a, "consult"),
        caller_hash(&b, "consult"),
        "changing a default implementation must re-key its perform sites"
    );
}

#[test]
fn same_signature_methods_are_distinct_when_bodies_differ() {
    // Without names in method identity, two same-signature methods are
    // told apart by their default implementations. A handler arm for one
    // must not catch performs of the other.
    let module = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000006) ability Fortune {
          fn tell(): Number { 4 }
          fn guess(): Number { 7 }
        }
        fn use_tell(): Number with Fortune { Fortune::tell!() }
        fn use_guess(): Number with Fortune { Fortune::guess!() }
        ",
    );
    assert_ne!(
        caller_hash(&module, "use_tell"),
        caller_hash(&module, "use_guess"),
        "same-signature methods with different bodies must be distinct"
    );
}

#[test]
fn unrelated_declarations_do_not_move_ability_hashes() {
    let a = compile(
        r"
        unique(AB100000-0000-0000-0000-000000000007) ability Fortune {
          fn tell(): Number { 4 }
        }
        fn consult(): Number with Fortune { Fortune::tell!() }
        ",
    );
    let b = compile(
        r"
        fn unrelated(): Number { 99 }
        unique(AB100000-0000-0000-0000-000000000007) ability Fortune {
          fn tell(): Number { 4 }
        }
        fn consult(): Number with Fortune { Fortune::tell!() }
        ",
    );
    let hashes_a = named_hashes(&a);
    let hashes_b = named_hashes(&b);
    assert_eq!(hashes_a["consult"], hashes_b["consult"]);
}
