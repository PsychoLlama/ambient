//! A builder rejects an ill-formed tree. See `docs/spec/trees.md`.

use ambient::builder::{self as b, BuildError, ty};

#[test]
fn names_must_be_identifiers() {
    for name in ["", "1x", "a b", "a-b"] {
        let module = b::module().define(b::function(name).body(b::null()));
        assert_eq!(
            module.build(),
            Err(BuildError::InvalidName(name.to_owned()))
        );
    }
}

#[test]
fn a_module_defines_a_name_once_per_namespace() {
    let twice = b::module()
        .define(b::function("f").body(b::null()))
        .define(b::function("f").body(b::int(1)));
    assert_eq!(
        twice.build(),
        Err(BuildError::DuplicateDefinition("f".to_owned()))
    );

    let once_each = b::module()
        .define(b::function("f").body(b::null()))
        .define(b::type_("f", ty::int()));
    assert!(once_each.build().is_ok());
}

#[test]
fn parameters_and_fields_are_unique() {
    let params = b::module().define(
        b::function("f")
            .param("x", ty::int())
            .param("x", ty::int())
            .body(b::null()),
    );
    assert!(matches!(
        params.build(),
        Err(BuildError::DuplicateParameter { .. })
    ));

    let fields =
        b::module().define(b::function("f").body(b::record([("x", b::int(1)), ("x", b::int(2))])));
    assert_eq!(
        fields.build(),
        Err(BuildError::DuplicateField("x".to_owned()))
    );

    let ty = b::module().define(b::type_(
        "T",
        ty::record([("x", ty::int()), ("x", ty::int())]),
    ));
    assert_eq!(ty.build(), Err(BuildError::DuplicateField("x".to_owned())));
}

#[test]
fn an_import_names_something_inside_a_package() {
    assert!(matches!(
        b::module().import("pkg").build(),
        Err(BuildError::ImportTooShort(_))
    ));
    assert_eq!(
        b::module().import("pkg.a.x").import("pkg.b.x").build(),
        Err(BuildError::DuplicateDefinition("x".to_owned())),
    );
}

#[test]
fn an_omitted_return_type_is_null() {
    let module = b::module().define(b::function("f").body(b::null())).build();
    let explicit = b::module()
        .define(b::function("f").returns(ty::null()).body(b::null()))
        .build();
    assert_eq!(module, explicit);
}

#[test]
fn a_tuple_has_two_or_more_members() {
    let returning = |body| b::module().define(b::function("f").body(body)).build();

    assert!(returning(b::tuple([b::int(1), b::int(2)])).is_ok());
    assert_eq!(
        returning(b::tuple([b::int(1)])),
        Err(BuildError::ShortTuple)
    );
    assert_eq!(returning(b::tuple([])), Err(BuildError::ShortTuple));
    // An all gives a tuple of what its branches give.
    assert_eq!(returning(b::all([b::int(1)])), Err(BuildError::ShortTuple));

    let ty = b::module().define(b::type_("T", ty::tuple([ty::int()])));
    assert_eq!(ty.build(), Err(BuildError::ShortTuple));
}
