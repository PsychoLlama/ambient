//! Snapshot-diff unit tests: identical ⇒ empty, body-only vs interface
//! module changes, the deploy rebinding rule over item bindings, module
//! add/remove, and deterministic JSON.

use super::snapshot::{BuildManifest, MANIFEST_VERSION, ManifestModule};
use super::*;

/// A manifest module with a name binding and a signature, sized to the diff
/// axes the tests exercise.
fn module(
    name: &str,
    ast: u8,
    iface: u8,
    binding: Option<(&str, u8, Option<&str>)>,
    objects: &[u8],
) -> ManifestModule {
    let (names, signatures) = match binding {
        Some((n, h, sig)) => (
            vec![(n.to_string(), [h; 32])],
            sig.map(|s| vec![(n.to_string(), s.to_string())])
                .unwrap_or_default(),
        ),
        None => (vec![], vec![]),
    };
    ManifestModule {
        module: name.to_string(),
        resolved_ast_hash: [ast; 32],
        interface_hash: [iface; 32],
        deps: vec![],
        objects: objects.iter().map(|b| [*b; 32]).collect(),
        names,
        signatures,
        cache_key: [0u8; 32],
        consumed_links: vec![],
        migrations: vec![],
        lambda_parents: vec![],
        entry_point: None,
        source_path: String::new(),
        items: vec![],
    }
}

fn manifest(modules: Vec<ManifestModule>) -> BuildManifest {
    BuildManifest {
        version: MANIFEST_VERSION,
        package_name: "demo".to_string(),
        dispatch_surface_hash: [0u8; 32],
        natives_contract_hash: [0u8; 32],
        core_cache_key: [0u8; 32],
        modules,
    }
}

#[test]
fn identical_snapshots_diff_empty() {
    let m = manifest(vec![module(
        "workspace::demo::main",
        1,
        1,
        Some(("workspace::demo::main::run", 9, Some("() -> Number"))),
        &[9],
    )]);
    let diff = diff_manifests(&m, &m);
    assert!(
        diff.is_empty(),
        "identical snapshots must diff empty: {diff:?}"
    );
    // Empty JSON has empty collections.
    let json = serde_json::to_value(&diff).expect("json");
    assert_eq!(json["modules"]["changed"], serde_json::json!([]));
    assert_eq!(json["bindings"]["rebound"], serde_json::json!([]));
    assert_eq!(json["objects"]["added"], serde_json::json!([]));
}

#[test]
fn body_only_edit_is_a_body_change_and_rebinds_the_item() {
    // Same interface hash, different AST hash and different object hash — a
    // private/body edit that keeps the same public signature.
    let a = manifest(vec![module(
        "workspace::demo::main",
        1,
        7,
        Some(("workspace::demo::main::run", 9, Some("() -> Number"))),
        &[9],
    )]);
    let b = manifest(vec![module(
        "workspace::demo::main",
        2,
        7,
        Some(("workspace::demo::main::run", 10, Some("() -> Number"))),
        &[10],
    )]);
    let diff = diff_manifests(&a, &b);

    assert_eq!(diff.modules.changed.len(), 1);
    assert_eq!(diff.modules.changed[0].module, "workspace::demo::main");
    assert!(
        !diff.modules.changed[0].interface_changed,
        "interface hash held ⇒ body-only change"
    );
    // Same signature, changed hash ⇒ rebound.
    assert_eq!(diff.bindings.rebound, vec!["workspace::demo::main::run"]);
    assert!(diff.bindings.retired.is_empty());
    // Objects: 10 added, 9 removed.
    assert_eq!(diff.objects.added.len(), 1);
    assert_eq!(diff.objects.removed.len(), 1);
}

#[test]
fn signature_edit_retires_and_moves_the_interface() {
    let a = manifest(vec![module(
        "workspace::demo::main",
        1,
        7,
        Some(("workspace::demo::main::run", 9, Some("() -> Number"))),
        &[9],
    )]);
    let b = manifest(vec![module(
        "workspace::demo::main",
        2,
        8,
        Some(("workspace::demo::main::run", 10, Some("() -> String"))),
        &[10],
    )]);
    let diff = diff_manifests(&a, &b);

    assert!(
        diff.modules.changed[0].interface_changed,
        "signature moved the interface"
    );
    assert_eq!(diff.bindings.retired, vec!["workspace::demo::main::run"]);
    assert!(diff.bindings.rebound.is_empty());
}

#[test]
fn module_and_binding_add_remove() {
    let a = manifest(vec![module(
        "workspace::demo::old",
        1,
        1,
        Some(("workspace::demo::old::gone", 1, Some("() -> Number"))),
        &[1],
    )]);
    let b = manifest(vec![module(
        "workspace::demo::new",
        2,
        2,
        Some(("workspace::demo::new::fresh", 2, Some("() -> Number"))),
        &[2],
    )]);
    let diff = diff_manifests(&a, &b);

    assert_eq!(diff.modules.added, vec!["workspace::demo::new"]);
    assert_eq!(diff.modules.removed, vec!["workspace::demo::old"]);
    assert_eq!(diff.bindings.added, vec!["workspace::demo::new::fresh"]);
    assert_eq!(diff.bindings.removed, vec!["workspace::demo::old::gone"]);
    assert_eq!(diff.objects.added.len(), 1);
    assert_eq!(diff.objects.removed.len(), 1);
}

#[test]
fn json_is_deterministic() {
    let a = manifest(vec![module(
        "workspace::demo::main",
        1,
        7,
        Some(("workspace::demo::main::run", 9, Some("() -> Number"))),
        &[9],
    )]);
    let b = manifest(vec![module(
        "workspace::demo::main",
        2,
        7,
        Some(("workspace::demo::main::run", 10, Some("() -> Number"))),
        &[10],
    )]);
    let diff = diff_manifests(&a, &b);
    let first = serde_json::to_string(&diff).expect("json");
    let second = serde_json::to_string(&diff).expect("json");
    assert_eq!(first, second, "serialization must be stable");
}

#[test]
fn dispatch_symbols_are_excluded_from_bindings() {
    // A `<uuid>::method` dispatch symbol is content-addressed, never a
    // late-bound name — the binding diff ignores it (matching deploy).
    let sym = "11111111-1111-1111-1111-111111111111::show";
    let a = manifest(vec![module(
        "workspace::demo::main",
        1,
        1,
        Some((sym, 1, None)),
        &[1],
    )]);
    let b = manifest(vec![module(
        "workspace::demo::main",
        2,
        1,
        Some((sym, 2, None)),
        &[2],
    )]);
    let diff = diff_manifests(&a, &b);
    assert!(diff.bindings.rebound.is_empty());
    assert!(diff.bindings.retired.is_empty());
    assert!(diff.bindings.added.is_empty());
}
