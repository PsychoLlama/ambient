//! Paths, imports, and visibility. See `spec/modules.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::core::{Expr, Tree};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::symbol::ModuleId;
use ambient::workspace::{Workspace, WorkspaceError};
use support::{value, workspace};

fn math() -> ModuleBuilder {
    b::module()
        .define(
            b::function("one")
                .public()
                .returns(ty::int())
                .body(b::int(1)),
        )
        .define(b::function("secret").returns(ty::int()).body(b::int(2)))
}

fn calling(imports: ModuleBuilder, callee: &str) -> ModuleBuilder {
    imports.define(
        b::function("main")
            .returns(ty::int())
            .body(b::call(b::name(callee), [])),
    )
}

/// Checks `main` in `app:main` and returns the symbols it refers to.
fn references(workspace: &Workspace) -> Result<Vec<String>, Error> {
    let store = MemoryStore::new();
    let open = Engine::new(workspace, &store).check(&value("main", "main"))?;
    Ok(open.slots.iter().map(ToString::to_string).collect())
}

#[test]
fn an_import_names_a_definition() {
    let main = calling(b::module().import("pkg.util.math.one"), "one");
    let workspace = workspace([("app", "util.math", math()), ("app", "main", main)]);

    assert_eq!(references(&workspace).unwrap(), ["app:util.math.one"]);
}

#[test]
fn an_import_names_a_module_used_as_a_qualifier() {
    let main = calling(b::module().import("pkg.util.math"), "math.one");
    let workspace = workspace([("app", "util.math", math()), ("app", "main", main)]);

    assert_eq!(references(&workspace).unwrap(), ["app:util.math.one"]);
}

#[test]
fn an_alias_resolves_to_the_same_symbol() {
    let main = calling(b::module().import_as("pkg.util.math.one", "uno"), "uno");
    let workspace = workspace([("app", "util.math", math()), ("app", "main", main)]);

    assert_eq!(references(&workspace).unwrap(), ["app:util.math.one"]);
}

#[test]
fn a_dependency_is_reached_by_its_local_name() {
    let main = calling(b::module().import("numbers.math.one"), "one");
    let library = b::module().expose("math");
    let modules = [
        ("lib", "", library),
        ("lib", "math", math()),
        ("app", "main", main),
    ];
    let mut workspace = workspace(modules);
    workspace.depend("app", "numbers", "lib").unwrap();

    assert_eq!(references(&workspace).unwrap(), ["lib:math.one"]);
}

/// `app` depending on `lib`, whose library module is `library` and which
/// has a module `math`; `main` is `app`'s only module.
fn depending(library: ModuleBuilder, main: ModuleBuilder) -> Workspace {
    let modules = [
        ("lib", "", library),
        ("lib", "math", math()),
        ("app", "main", main),
    ];
    let mut workspace = workspace(modules);
    workspace.depend("app", "lib", "lib").unwrap();
    workspace
}

#[test]
fn a_module_its_library_does_not_list_is_internal() {
    let definition = calling(b::module().import("lib.math.one"), "one");
    let qualifier = calling(b::module().import("lib.math"), "math.one");

    for main in [definition, qualifier] {
        assert!(matches!(
            references(&depending(b::module(), main)),
            Err(Error::InternalModule { .. })
        ));
    }
}

#[test]
fn a_package_without_a_library_module_exposes_nothing() {
    let main = calling(b::module().import("lib.math.one"), "one");
    let mut workspace = workspace([("lib", "math", math()), ("app", "main", main)]);
    workspace.depend("app", "lib", "lib").unwrap();

    assert!(matches!(
        references(&workspace),
        Err(Error::InternalModule { .. })
    ));
}

#[test]
fn a_library_modules_definitions_are_reached_through_the_package() {
    let library = b::module().define(
        b::function("two")
            .public()
            .returns(ty::int())
            .body(b::int(2)),
    );
    let main = calling(b::module().import("lib.two"), "two");

    assert_eq!(references(&depending(library, main)).unwrap(), ["lib:.two"]);
}

#[test]
fn a_package_reaches_its_own_modules_whether_listed_or_not() {
    let main = calling(b::module().import("pkg.math.one"), "one");
    let modules = [
        ("app", "", b::module()),
        ("app", "math", math()),
        ("app", "main", main),
    ];

    assert_eq!(references(&workspace(modules)).unwrap(), ["app:math.one"]);
}

#[test]
fn a_library_module_cannot_define_the_name_of_a_top_level_module() {
    let library = b::module()
        .expose("math")
        .define(b::function("math").public().body(b::int(2)));
    let main = calling(b::module().import("lib.math"), "math.one");

    assert!(matches!(
        references(&depending(library, main)),
        Err(Error::DuplicateName { name, .. }) if name == "math"
    ));
}

#[test]
fn an_undeclared_package_is_out_of_reach() {
    let main = calling(b::module().import("lib.math.one"), "one");
    let workspace = workspace([("lib", "math", math()), ("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::UnknownPackage { name, .. }) if name == "lib"
    ));
}

#[test]
fn a_private_definition_cannot_be_imported() {
    let main = calling(b::module().import("pkg.math.secret"), "secret");
    let workspace = workspace([("app", "math", math()), ("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::PrivateImport { .. })
    ));
}

#[test]
fn a_private_definition_cannot_be_reached_through_its_module() {
    let main = calling(b::module().import("pkg.math"), "math.secret");
    let workspace = workspace([("app", "math", math()), ("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::PrivateName { .. })
    ));
}

#[test]
fn an_import_of_nothing_is_an_error() {
    let main = calling(b::module().import("pkg.math.two"), "two");
    let workspace = workspace([("app", "math", math()), ("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::UnknownImport { .. })
    ));
}

#[test]
fn an_import_cannot_take_a_name_the_module_defines() {
    let main = calling(b::module().import_as("pkg.math.one", "main"), "main");
    let workspace = workspace([("app", "math", math()), ("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::DuplicateName { .. })
    ));
}

#[test]
fn one_import_brings_in_a_name_from_every_namespace() {
    let shapes = b::module()
        .define(b::type_("point", ty::record([("x", ty::int())])).public())
        .define(
            b::function("point")
                .public()
                .returns(ty::named("point"))
                .body(b::record([("x", b::int(0))])),
        );
    let main = b::module().import("pkg.shapes.point").define(
        b::function("main")
            .returns(ty::named("point"))
            .body(b::call(b::name("point"), [])),
    );
    let workspace = workspace([("app", "shapes", shapes), ("app", "main", main)]);
    let store = MemoryStore::new();
    let open = Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .unwrap();

    // The type resolved too: it is transparent, so it leaves no slot, but
    // the body would not check against anything else.
    let shapes = ModuleId::new("app", "shapes");
    assert_eq!(open.slots, [shapes.value("point")]);
}

#[test]
fn a_path_segment_is_a_file_or_a_directory() {
    let mut workspace = workspace([("app", "util.math", math())]);

    assert_eq!(
        workspace.set(ModuleId::new("app", "util"), math().build().unwrap()),
        Err(WorkspaceError::FileAndDirectory(ModuleId::new(
            "app", "util"
        ))),
    );
    assert!(
        workspace
            .set(ModuleId::new("lib", "util"), math().build().unwrap())
            .is_ok()
    );
}

#[test]
fn locals_shadow_definitions_and_are_numbered_in_binding_order() {
    let module = b::module()
        .define(math().build().unwrap().definitions()[0].clone())
        .define(
            b::function("main")
                .param("one", ty::int())
                .returns(ty::int())
                .body(b::let_(
                    "x",
                    b::name("one"),
                    b::let_("x", b::name("x"), b::name("x")),
                )),
        );
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let open = Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .unwrap();

    let Tree::Function(function) = &open.tree else {
        panic!("not a function")
    };
    let local = |n| Box::new(Expr::Local(n));
    assert!(open.slots.is_empty());
    assert_eq!(
        function.body,
        Expr::Let {
            value: local(0),
            body: Box::new(Expr::Let {
                value: local(1),
                body: local(2),
            }),
        }
    );
}

#[test]
fn a_call_must_pass_as_many_arguments_as_the_header_declares() {
    let main = b::module()
        .define(b::function("one").returns(ty::int()).body(b::int(1)))
        .define(
            b::function("main")
                .returns(ty::int())
                .body(b::call(b::name("one"), [b::int(1)])),
        );
    let workspace = workspace([("app", "main", main)]);

    assert!(matches!(
        references(&workspace),
        Err(Error::Arity {
            expected: 0,
            found: 1,
            ..
        })
    ));
}
