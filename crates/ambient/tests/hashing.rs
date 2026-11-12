//! What is and is not content. See `spec/definitions.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::symbol::Symbol;
use support::{value, workspace};

fn hash_of(symbol: &Symbol, modules: &[(&str, ModuleBuilder)]) -> Hash {
    let mut workspace = ambient::workspace::Workspace::new();
    for (path, module) in modules {
        workspace
            .set(
                ambient::symbol::ModuleId::new("app", path),
                module.clone().build().unwrap(),
            )
            .unwrap();
    }
    let store = MemoryStore::new();
    Engine::new(&workspace, &store).hash(symbol).unwrap()
}

fn double(function: &str, param: &str) -> ModuleBuilder {
    b::module().define(
        b::function(function)
            .param(param, ty::int())
            .returns(ty::int())
            .body(b::name(param)),
    )
}

#[test]
fn names_are_not_content() {
    let original = hash_of(&value("main", "double"), &[("main", double("double", "x"))]);

    let renamed_param = hash_of(&value("main", "double"), &[("main", double("double", "y"))]);
    let renamed_function = hash_of(&value("main", "twice"), &[("main", double("twice", "x"))]);
    let moved = hash_of(
        &value("other.place", "double"),
        &[("other.place", double("double", "x"))],
    );

    assert_eq!(original, renamed_param);
    assert_eq!(original, renamed_function);
    assert_eq!(original, moved);
}

#[test]
fn docs_and_visibility_are_not_content() {
    let plain = b::function("f").returns(ty::int()).body(b::int(1));
    let documented = b::function("f")
        .public()
        .doc("One.")
        .returns(ty::int())
        .body(b::int(1));

    assert_eq!(
        hash_of(&value("main", "f"), &[("main", b::module().define(plain))]),
        hash_of(
            &value("main", "f"),
            &[("main", b::module().define(documented))]
        ),
    );
}

#[test]
fn literals_and_structure_are_content() {
    let one = b::module().define(b::function("f").returns(ty::int()).body(b::int(1)));
    let two = b::module().define(b::function("f").returns(ty::int()).body(b::int(2)));
    let float = b::module().define(b::function("f").returns(ty::float()).body(b::float(1.0)));

    let one = hash_of(&value("main", "f"), &[("main", one)]);
    assert_ne!(one, hash_of(&value("main", "f"), &[("main", two)]));
    assert_ne!(one, hash_of(&value("main", "f"), &[("main", float)]));
}

#[test]
fn a_signature_is_content() {
    let int = b::module().define(b::function("f").param("x", ty::int()).body(b::null()));
    let string = b::module().define(b::function("f").param("x", ty::string()).body(b::null()));

    assert_ne!(
        hash_of(&value("main", "f"), &[("main", int)]),
        hash_of(&value("main", "f"), &[("main", string)]),
    );
}

#[test]
fn record_type_field_order_is_not_content() {
    let xy = b::module().define(b::type_(
        "P",
        ty::record([("x", ty::int()), ("y", ty::float())]),
    ));
    let yx = b::module().define(b::type_(
        "P",
        ty::record([("y", ty::float()), ("x", ty::int())]),
    ));

    assert_eq!(
        hash_of(&support::ty("main", "P"), &[("main", xy)]),
        hash_of(&support::ty("main", "P"), &[("main", yx)]),
    );
}

#[test]
fn record_construction_order_is_content() {
    let record = |fields| {
        b::module().define(
            b::function("f")
                .returns(ty::unknown())
                .body(b::record(fields)),
        )
    };
    let xy = record([("x", b::int(1)), ("y", b::int(2))]);
    let yx = record([("y", b::int(2)), ("x", b::int(1))]);

    assert_ne!(
        hash_of(&value("main", "f"), &[("main", xy)]),
        hash_of(&value("main", "f"), &[("main", yx)]),
    );
}

#[test]
fn a_dependency_is_content_by_hash_not_by_name() {
    let program = |helper: &str, result: i64| {
        b::module()
            .define(b::function(helper).returns(ty::int()).body(b::int(result)))
            .define(
                b::function("main")
                    .returns(ty::int())
                    .body(b::call(b::name(helper), [])),
            )
    };
    let main = value("main", "main");

    let original = hash_of(&main, &[("main", program("helper", 1))]);
    assert_eq!(original, hash_of(&main, &[("main", program("renamed", 1))]));
    assert_ne!(original, hash_of(&main, &[("main", program("helper", 2))]));
}

#[test]
fn equal_content_has_one_hash_whatever_its_symbol() {
    let module = b::module()
        .define(b::function("a").returns(ty::int()).body(b::int(7)))
        .define(b::function("b").returns(ty::int()).body(b::int(7)));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);

    assert_eq!(
        engine.hash(&value("main", "a")).unwrap(),
        engine.hash(&value("main", "b")).unwrap(),
    );
}

/// `even` and `odd` call each other; `entry` calls `even`.
fn parity(even: &str, odd: &str, swap: bool) -> ModuleBuilder {
    let even_def = b::function(even)
        .param("n", ty::int())
        .returns(ty::bool())
        .body(b::call(b::name(odd), [b::name("n")]));
    let odd_def = b::function(odd)
        .param("n", ty::int())
        .returns(ty::bool())
        .body(b::call(b::name(even), [b::int(0)]));
    let module = if swap {
        b::module().define(odd_def).define(even_def)
    } else {
        b::module().define(even_def).define(odd_def)
    };
    module.define(
        b::function("entry")
            .returns(ty::bool())
            .body(b::call(b::name(even), [b::int(4)])),
    )
}

#[test]
fn a_group_is_ordered_by_content() {
    let hashes = |even: &str, odd: &str, swap| {
        let module = [("main", parity(even, odd, swap))];
        (
            hash_of(&value("main", even), &module),
            hash_of(&value("main", odd), &module),
            hash_of(&value("main", "entry"), &module),
        )
    };

    let original = hashes("even", "odd", false);
    assert_ne!(original.0, original.1);
    assert_eq!(original, hashes("even", "odd", true), "declaration order");
    assert_eq!(original, hashes("zzz", "aaa", false), "names");
}

#[test]
fn a_group_member_hash_does_not_depend_on_where_the_walk_starts() {
    let workspace = workspace([("app", "main", parity("even", "odd", false))]);
    let store = MemoryStore::new();

    let from_even = Engine::new(&workspace, &store);
    let even = from_even.hash(&value("main", "even")).unwrap();
    let from_odd = Engine::new(&workspace, &store);
    from_odd.hash(&value("main", "odd")).unwrap();

    assert_eq!(even, from_odd.hash(&value("main", "even")).unwrap());
}

#[test]
fn indistinguishable_group_members_share_a_hash() {
    let ping = |name: &str, other: &str| {
        b::function(name)
            .returns(ty::int())
            .body(b::call(b::name(other), []))
    };
    let module = b::module().define(ping("a", "b")).define(ping("b", "a"));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);

    assert_eq!(
        engine.hash(&value("main", "a")).unwrap(),
        engine.hash(&value("main", "b")).unwrap(),
    );
}

#[test]
fn self_reference_is_hashed_as_a_group_of_one() {
    let looping = |name: &str| {
        b::module().define(
            b::function(name)
                .returns(ty::int())
                .body(b::call(b::name(name), [])),
        )
    };
    let constant = b::module().define(b::function("f").returns(ty::int()).body(b::int(0)));

    let original = hash_of(&value("main", "f"), &[("main", looping("f"))]);
    assert_eq!(
        original,
        hash_of(&value("main", "g"), &[("main", looping("g"))])
    );
    assert_ne!(
        original,
        hash_of(&value("main", "f"), &[("main", constant)])
    );
}

#[test]
fn recursive_types_hash() {
    let module = b::module()
        .define(b::type_(
            "Tree",
            ty::record([("children", ty::named("Forest"))]),
        ))
        .define(b::type_(
            "Forest",
            ty::record([("first", ty::named("Tree"))]),
        ));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);

    assert_ne!(
        engine.hash(&support::ty("main", "Tree")).unwrap(),
        engine.hash(&support::ty("main", "Forest")).unwrap(),
    );
}
