//! Package-mount behavior of the registry (workspace builds): mounted
//! module identity, its inverse, and `pkg`/`super`/`self`/`::package`
//! path resolution against the mount boundary. Split from `tests` for
//! the per-file line budget.

use super::*;
use crate::ast::{Expr, FunctionDef, Item, Span};
use crate::module_path::ResolutionError;

fn make_function(name: &str, is_public: bool) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        }),
        Span::default(),
    )
}

fn make_module(name: &str, items: Vec<Item>) -> Arc<Module> {
    Arc::new(Module {
        name: Arc::from(name),
        doc: None,
        items,
    })
}

fn mp(segments: &[&str]) -> ModulePath {
    ModulePath::from_str_segments(segments).unwrap()
}

/// A two-package mounted registry: `app` (root module + `client`) and
/// `lib` (root module + `utils`).
fn mounted_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    registry.add_mount("app");
    registry.add_mount("lib");
    // Each package root main.ab collapses to the mount as a directory
    // module, exactly like `<dir>/main.ab`.
    registry.register_module(&mp(&["app"]), make_module("app", vec![]), true);
    registry.register_module(
        &mp(&["app", "client"]),
        make_module("client", vec![make_function("start", true)]),
        false,
    );
    registry.register_module(
        &mp(&["lib"]),
        make_module("lib", vec![make_function("greet", true)]),
        true,
    );
    registry.register_module(
        &mp(&["lib", "utils"]),
        make_module("utils", vec![make_function("helper", true)]),
        false,
    );
    registry
}

#[test]
fn mounted_module_ids_scope_to_their_package() {
    let registry = mounted_registry();

    // The mount itself is the package's root module.
    let root = registry.module_id(&mp(&["lib"]));
    assert_eq!(root.scope, crate::fqn::Scope::Workspace(Arc::from("lib")));
    assert!(root.path.is_empty());
    assert_eq!(root.to_string(), "workspace::lib");

    // A mounted submodule strips the mount segment.
    let utils = registry.module_id(&mp(&["lib", "utils"]));
    assert_eq!(utils.to_string(), "workspace::lib::utils");

    // Core stays builtin; unmounted paths stay under the bare workspace.
    let core = registry.module_id(&mp(&["core", "option"]));
    assert_eq!(core.scope, crate::fqn::Scope::Builtin);
}

#[test]
fn module_path_of_inverts_module_id_under_mounts() {
    let registry = mounted_registry();
    for path in [mp(&["lib"]), mp(&["lib", "utils"]), mp(&["app", "client"])] {
        let id = registry.module_id(&path);
        assert_eq!(registry.module_path_of(&id), Some(path.clone()));
        assert_eq!(registry.module_key(&id), path.to_string());
    }
}

#[test]
fn workspace_use_resolves_across_mounts() {
    let registry = mounted_registry();

    // `use ::lib::utils;` from app's root module.
    let target = registry
        .resolve_use_path(
            &mp(&["app"]),
            &UsePrefix::Workspace,
            &[Arc::from("lib"), Arc::from("utils")],
        )
        .unwrap();
    assert_eq!(target, mp(&["lib", "utils"]));
    assert!(registry.lookup_symbol(&target, "helper").is_ok());

    // `use ::lib;` names the package root module itself.
    let root = registry
        .resolve_use_path(&mp(&["app"]), &UsePrefix::Workspace, &[Arc::from("lib")])
        .unwrap();
    assert!(registry.lookup_symbol(&root, "greet").is_ok());
}

#[test]
fn pkg_use_anchors_at_the_mount() {
    let registry = mounted_registry();

    let target = registry
        .resolve_use_path(
            &mp(&["app", "client"]),
            &UsePrefix::Pkg,
            &[Arc::from("client")],
        )
        .unwrap();
    assert_eq!(target, mp(&["app", "client"]));
}

#[test]
fn super_never_escapes_the_mount() {
    let registry = mounted_registry();

    // From `app/client.ab`, one `super` would step above the package root.
    let err = registry
        .resolve_use_path(
            &mp(&["app", "client"]),
            &UsePrefix::Super(1),
            &[Arc::from("lib")],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        RegistryError::PathResolution(ResolutionError::EscapedPackageRoot)
    ));
}

#[test]
fn self_from_mounted_root_stays_in_package() {
    let registry = mounted_registry();

    // The package root module is a directory module: `self::client` from
    // app's main.ab names its own top-level module, not a sibling package.
    let target = registry
        .resolve_use_path(&mp(&["app"]), &UsePrefix::Self_, &[Arc::from("client")])
        .unwrap();
    assert_eq!(target, mp(&["app", "client"]));
}
